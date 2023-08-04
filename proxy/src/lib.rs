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
http reverse proxy trillium handler



*/

use full_duplex_async_copy::full_duplex_copy;
use size::{Base, Size};
use trillium::{
    async_trait, conn_try, Conn, Handler, KnownHeaderName,
    Status::{NotFound, SwitchingProtocols},
    Upgrade,
};
pub use trillium_client::Client;
use trillium_forwarding::Forwarded;
use trillium_http::HeaderValue;
use url::Url;

/**
the proxy handler
*/
#[derive(Debug)]
pub struct Proxy {
    target: Url,
    client: Client,
    pass_through_not_found: bool,
    halt: bool,
    via_pseudonym: Option<String>,
}

impl Proxy {
    /**
    construct a new proxy handler that sends all requests to the url
    provided.  if the url contains a path, the inbound request path
    will be joined onto the end.

    ```
    use trillium_smol::ClientConfig;
    use trillium_proxy::Proxy;

    let proxy = Proxy::new(ClientConfig::default(), "http://docs.trillium.rs/trillium_proxy");
    ```

     */
    pub fn new(client: impl Into<Client>, target: impl TryInto<Url>) -> Self {
        let url = match target.try_into() {
            Ok(url) => url,
            Err(_) => panic!("could not convert proxy target into a url"),
        };

        assert!(!url.cannot_be_a_base(), "{url} cannot be a base");

        Self {
            target: url,
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
    pub fn with_via_pseudonym(mut self, via_pseudonym: String) -> Self {
        self.via_pseudonym = Some(via_pseudonym);
        self
    }
}

struct UpstreamUpgrade(Upgrade);

#[async_trait]
impl Handler for Proxy {
    async fn run(&self, mut conn: Conn) -> Conn {
        let mut request_url = conn_try!(self.target.join(conn.path()), conn);
        let querystring = conn.querystring();
        if !querystring.is_empty() {
            request_url.set_query(Some(querystring));
        }
        log::debug!("proxying to {}", request_url);

        // need a better solution for streaming request bodies through
        // the proxy, but http-types::Body needs to be 'static. Fixing
        // this probably will entail moving away from http-types::Body
        // for outbound bodies.
        //
        // this is very inefficient and possibly unscalable in
        // situations where request bodies are large. there is no
        // reason that we couldn't have another lifetime on client
        // conn here, though
        let client_body_content = conn_try!(conn.request_body().await.read_bytes().await, conn);

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

        let Ok(mut client_conn) = self
            .client
            .build_conn(conn.method(), request_url)
            .with_headers(request_headers)
            .with_body(client_body_content)
            .await
        else {
            return conn.with_status(500).halt();
        };

        let conn = match client_conn.status() {
            Some(SwitchingProtocols) => {
                conn.headers_mut()
                    .extend(std::mem::take(client_conn.response_headers_mut()).into_iter());

                conn.with_state(UpstreamUpgrade(client_conn.into()))
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

            _ => unreachable!(),
        };

        if self.halt {
            conn.halt()
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state.get::<UpstreamUpgrade>().is_some()
    }

    async fn upgrade(&self, mut upgrade: Upgrade) {
        let upstream = upgrade.state.take::<UpstreamUpgrade>().unwrap().0;
        let downstream = upgrade;
        match full_duplex_copy(upstream, downstream).await {
            Err(e) => log::error!("{}:{} {:?}", file!(), line!(), e),
            Ok((up, down)) => log::debug!("wrote {} up and {} down", bytes(up), bytes(down)),
        }
    }
}

fn bytes(bytes: u64) -> String {
    Size::from_bytes(bytes)
        .format()
        .with_base(Base::Base10)
        .to_string()
}
