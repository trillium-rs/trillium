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

use event_listener::Event;
use full_duplex_async_copy::full_duplex_copy;
use futures_lite::{future::zip, AsyncRead};
use size::{Base, Size};
use std::{
    future::{Future, IntoFuture},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use trillium::{
    async_trait, conn_try, Conn, Handler, KnownHeaderName,
    Status::{NotFound, SwitchingProtocols},
    Upgrade,
};
use trillium_forwarding::Forwarded;
use trillium_http::{Body, HeaderValue, Method, Status};
use url::Url;

pub use trillium_client::Client;

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

struct BodyProxyReader {
    reader: sluice::pipe::PipeReader,
    started: Option<Arc<(Event, AtomicBool)>>,
}

impl Drop for BodyProxyReader {
    fn drop(&mut self) {
        // if we haven't started yet, notify the copy future that we're not going to
        if let Some(started) = self.started.take() {
            started.0.notify(usize::MAX);
        }
    }
}

impl AsyncRead for BodyProxyReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(started) = self.started.take() {
            started.1.store(true, Ordering::SeqCst);
            started.0.notify(usize::MAX);
        }
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}

fn body_proxy(conn: &mut Conn) -> (impl Future<Output = ()> + Send + Sync + '_, Body) {
    let started = Arc::new((Event::new(), AtomicBool::from(false)));
    let started_clone = started.clone();
    let (reader, writer) = sluice::pipe::pipe();
    let len = conn
        .request_headers()
        .get_str(KnownHeaderName::ContentLength)
        .and_then(|s| s.parse().ok());

    (
        async move {
            log::trace!("waiting to stream request body");
            started_clone.0.listen().await;
            if started_clone.1.load(Ordering::SeqCst) {
                log::trace!("started to stream request body");
                let received_body = conn.request_body().await;
                match trillium_http::copy(received_body, writer, 4).await {
                    Ok(streamed) => {
                        log::info!("streamed {} request body bytes", bytes(streamed))
                    }
                    Err(e) => log::error!("request body stream error: {e}"),
                };
            } else {
                log::trace!("not streaming request body");
            }
        },
        Body::new_streaming(
            BodyProxyReader {
                started: Some(started),
                reader,
            },
            len,
        ),
    )
}

#[async_trait]
impl Handler for Proxy {
    async fn run(&self, mut conn: Conn) -> Conn {
        let mut request_url = conn_try!(self.target.join(conn.path()), conn);
        let querystring = conn.querystring();
        if !querystring.is_empty() {
            request_url.set_query(Some(querystring));
        }
        log::debug!("proxying to {}", request_url);

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
            let (body_fut, request_body) = body_proxy(&mut conn);

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
