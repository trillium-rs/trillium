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
use std::convert::TryInto;
use trillium::{
    async_trait, conn_try, Conn, Handler,
    KnownHeaderName::{Connection, Host},
    Status::{NotFound, SwitchingProtocols},
};
use trillium_http::{transport::BoxedTransport, Upgrade};
use url::Url;

pub use trillium_client::{Client, Connector};

/**
the proxy handler
*/
#[derive(Debug)]
pub struct Proxy<C: Connector> {
    target: Url,
    client: Client<C>,
    pass_through_not_found: bool,
    halt: bool,
}

impl<C: Connector + Clone> Proxy<C> {
    /**
    construct a new proxy handler that sends all requests to the url
    provided.  if the url contains a path, the inbound request path
    will be joined onto the end.

    ```
    use trillium_smol::ClientConfig;
    use trillium_proxy::Proxy;

    let proxy = Proxy::<TcpConnector>::new("http://docs.trillium.rs/trillium_proxy");
    ```

     */
    pub fn new(config: C, target: impl TryInto<Url>) -> Self {
        let url = match target.try_into() {
            Ok(url) => url,
            Err(_) => panic!("could not convert proxy target into a url"),
        };

        assert!(!url.cannot_be_a_base(), "{url} cannot be a base");

        Self {
            target: url,
            client: Client::new(config).with_default_pool(),
            pass_through_not_found: true,
            halt: true,
        }
    }

    /**
    chainable constructor to specfiy a [`Client`] to use. This is
    useful if the application already is using trillium_client for
    other requests, as it will reuse the same connection pool and
    connector config.

    note that this clears out any changes made with
    [`Proxy::with_config`]. configure the client directly if you are
    providing one

    ```
    use trillium_smol::{TcpConnector, ClientConfig};
    use trillium_proxy::{Proxy, Client};

    let client = Client::new().with_default_pool();
    let proxy = Proxy::<TcpConnector>::new("http://trillium.rs")
        .with_client(client); //<-
    ```

    ```
    // sharing a client with other trillium handlers
    # use trillium_smol::{TcpConnector, ClientConfig};
    # use trillium_proxy::{Proxy, Client};
    use trillium::State;

    let client = Client::new().with_default_pool();
    let handler = (
        State::new(client.clone()),
        Proxy::<TcpConnector>::new("http://trillium.rs").with_client(client)
    );
    ```
     */
    pub fn with_client(mut self, client: Client<C>) -> Self {
        self.client = client;
        self
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
    # use trillium_smol::TcpConnector;
    # use trillium_proxy::Proxy;
    let proxy = Proxy::<TcpConnector>::new("http://trillium.rs")
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
    # use trillium_smol::TcpConnector;
    # use trillium_proxy::Proxy;
    let proxy = Proxy::<TcpConnector>::new("http://trillium.rs")
        .without_halting();
    ```
    */

    pub fn without_halting(mut self) -> Self {
        self.halt = false;
        self
    }
}

struct UpstreamUpgrade<T>(Upgrade<T>);

#[async_trait]
impl<C: Connector + Clone> Handler for Proxy<C> {
    async fn run(&self, mut conn: Conn) -> Conn {
        let mut request_url = conn_try!(self.target.clone().join(conn.path()), conn);
        let querystring = conn.querystring();
        if !querystring.is_empty() {
            request_url.set_query(Some(conn.querystring()));
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

        let headers = conn
            .headers()
            .clone()
            .without_header(Host)
            .with_inserted_header(Connection, "keep-alive");

        let mut client_conn = conn_try!(
            self.client
                .build_conn(conn.method(), request_url)
                .with_headers(headers)
                .with_body(client_body_content)
                .await,
            conn
        );

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
                    .extend(std::mem::take(client_conn.response_headers_mut()));
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

    fn has_upgrade(&self, upgrade: &Upgrade<BoxedTransport>) -> bool {
        upgrade
            .state
            .get::<UpstreamUpgrade<C::Transport>>()
            .is_some()
    }

    async fn upgrade(&self, mut upgrade: trillium::Upgrade) {
        let upstream = upgrade
            .state
            .take::<UpstreamUpgrade<C::Transport>>()
            .unwrap()
            .0;
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
