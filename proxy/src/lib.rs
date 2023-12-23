#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
http reverse and forward proxy trillium handler

*/

mod body_streamer;
mod forward_proxy_connect;
pub mod upstream;

use body_streamer::stream_body;
use full_duplex_async_copy::full_duplex_copy;
use futures_lite::future::zip;
use size::{Base, Size};
use std::{borrow::Cow, fmt::Debug, future::IntoFuture};
use trillium::{
    async_trait, Conn, Handler, KnownHeaderName,
    Status::{NotFound, SwitchingProtocols},
    Upgrade,
};
use trillium_forwarding::Forwarded;
use trillium_http::{HeaderValue, Method, Status};
use upstream::{IntoUpstreamSelector, UpstreamSelector};

pub use forward_proxy_connect::ForwardProxyConnect;
pub use trillium_client::{Client, Connector};
pub use url::Url;

/// constructs a new [`Proxy`]. alias of [`Proxy::new`]
pub fn proxy<I>(client: impl Into<Client>, upstream: I) -> Proxy<I::UpstreamSelector>
where
    I: IntoUpstreamSelector,
{
    Proxy::new(client, upstream)
}

/**
the proxy handler
*/
#[derive(Debug)]
pub struct Proxy<U> {
    upstream: U,
    client: Client,
    pass_through_not_found: bool,
    halt: bool,
    via_pseudonym: Option<Cow<'static, str>>,
}

impl<U: UpstreamSelector> Proxy<U> {
    /**
    construct a new proxy handler that sends all requests to the upstream
    provided

    ```
    use trillium_smol::ClientConfig;
    use trillium_proxy::Proxy;

    let proxy = Proxy::new(ClientConfig::default(), "http://docs.trillium.rs/trillium_proxy");
    ```

     */
    pub fn new<I>(client: impl Into<Client>, upstream: I) -> Self
    where
        I: IntoUpstreamSelector<UpstreamSelector = U>,
    {
        Self {
            upstream: upstream.into_upstream(),
            client: client.into(),
            pass_through_not_found: true,
            halt: true,
            via_pseudonym: None,
        }
    }

    /**
    chainable constructor to set the 404 Not Found handling
    behavior. By default, this proxy will pass through the trillium
    Conn unmodified if the proxy response is a 404 not found, allowing
    it to be chained in a tuple handler. To modify this behavior, call
    proxy_not_found, and the full 404 response will be forwarded. The
    Conn will be halted unless [`Proxy::without_halting`] was
    configured

    ```
    # use trillium_smol::ClientConfig;
    # use trillium_proxy::Proxy;
    let proxy = Proxy::new(ClientConfig::default(), "http://trillium.rs")
        .proxy_not_found();
    ```
    */
    pub fn proxy_not_found(mut self) -> Self {
        self.pass_through_not_found = false;
        self
    }

    /**
    The default behavior for this handler is to halt the conn on any
    response other than a 404. If [`Proxy::proxy_not_found`] has been
    configured, the default behavior for all response statuses is to
    halt the trillium conn. To change this behavior, call
    without_halting when constructing the proxy, and it will not halt
    the conn. This is useful when passing the proxy reply through
    [`trillium_html_rewriter`](https://docs.trillium.rs/trillium_html_rewriter).

    ```
    # use trillium_smol::ClientConfig;
    # use trillium_proxy::Proxy;
    let proxy = Proxy::new(ClientConfig::default(), "http://trillium.rs")
        .without_halting();
    ```
    */
    pub fn without_halting(mut self) -> Self {
        self.halt = false;
        self
    }

    /// populate the pseudonym for a
    /// [`Via`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Via)
    /// header. If no pseudonym is provided, no via header will be
    /// inserted.
    pub fn with_via_pseudonym(mut self, via_pseudonym: impl Into<Cow<'static, str>>) -> Self {
        self.via_pseudonym = Some(via_pseudonym.into());
        self
    }

    }
}

#[derive(Debug)]
struct UpstreamUpgrade(Upgrade);

#[async_trait]
impl<U: UpstreamSelector> Handler for Proxy<U> {
    async fn init(&mut self, _info: &mut trillium::Info) {
        log::info!("proxying to {:?}", self.upstream);
    }

    async fn run(&self, mut conn: Conn) -> Conn {
        let Some(request_url) = self.upstream.determine_upstream(&mut conn) else {
            return conn;
        };

        log::debug!("proxying to {}", request_url.as_str());

        let mut forwarded = Forwarded::from_headers(conn.request_headers())
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_owned();

        if let Some(peer_ip) = conn.peer_ip() {
            forwarded.add_for(peer_ip.to_string());
        };

        if let Some(host) = conn.inner().host() {
            forwarded.set_host(host);
        }

        let mut request_headers = conn
            .request_headers()
            .clone()
            .without_headers([
                KnownHeaderName::Host,
                KnownHeaderName::XforwardedBy,
                KnownHeaderName::XforwardedFor,
                KnownHeaderName::XforwardedHost,
                KnownHeaderName::XforwardedProto,
                KnownHeaderName::XforwardedSsl,
                KnownHeaderName::AcceptEncoding,
            ])
            .with_inserted_header(KnownHeaderName::Connection, "keep-alive")
            .with_inserted_header(KnownHeaderName::Forwarded, forwarded.to_string());

        if let Some(via) = &self.via_pseudonym {
            let new_via = format!("{} {}", conn.inner().http_version(), via);
            let via = match request_headers.get_values(KnownHeaderName::Via) {
                Some(old_via) => format!(
                    "{new_via}, {}",
                    old_via
                        .iter()
                        .filter_map(HeaderValue::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                ),

                None => new_via,
            };

            request_headers.insert(KnownHeaderName::Via, via);
        };

        let method = conn.method();
        let conn_result = if method == Method::Get {
            self.client
                .get(request_url)
                .with_headers(request_headers)
                .await
        } else {
            let (body_fut, request_body) = stream_body(&mut conn);

            let client_fut = self
                .client
                .build_conn(method, request_url)
                .with_headers(request_headers)
                .with_body(request_body)
                .into_future();

            zip(body_fut, client_fut).await.1
        };

        let mut client_conn = match conn_result {
            Ok(client_conn) => client_conn,
            Err(e) => {
                return conn
                    .with_status(Status::ServiceUnavailable)
                    .halt()
                    .with_state(e);
            }
        };

        let conn = match client_conn.status() {
            Some(SwitchingProtocols) => {
                conn.headers_mut()
                    .extend(std::mem::take(client_conn.response_headers_mut()));

                conn.with_state(UpstreamUpgrade(Upgrade::from(client_conn)))
                    .with_status(SwitchingProtocols)
            }

            Some(NotFound) if self.pass_through_not_found => {
                client_conn.recycle().await;
                return conn;
            }

            Some(status) => {
                conn.headers_mut()
                    .append_all(client_conn.response_headers().clone());
                conn.with_body(client_conn).with_status(status)
            }

            None => return conn.with_status(Status::ServiceUnavailable).halt(),
        };

        if self.halt {
            conn.halt()
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state.contains::<UpstreamUpgrade>()
    }

    async fn upgrade(&self, mut upgrade: Upgrade) {
        let Some(UpstreamUpgrade(upstream)) = upgrade.state.take() else {
            return;
        };
        let downstream = upgrade;
        match full_duplex_copy(upstream, downstream).await {
            Err(e) => log::error!("upgrade stream error: {:?}", e),
            Ok((up, down)) => {
                log::debug!("streamed upgrade {} up and {} down", bytes(up), bytes(down))
            }
        }
    }
}

fn bytes(bytes: u64) -> String {
    Size::from_bytes(bytes)
        .format()
        .with_base(Base::Base10)
        .to_string()
}
