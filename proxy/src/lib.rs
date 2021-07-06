#![forbid(unsafe_code)]
#![deny(
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
use size::{Base, Size, Style};
use std::convert::TryInto;
use trillium::{
    async_trait, conn_try,
    http_types::{StatusCode, Url},
    Conn, Handler,
};
use trillium_http::{transport::BoxedTransport, Upgrade};
use StatusCode::{NotFound, SwitchingProtocols};

pub use trillium_client::{AsConnector, Client, Connector};

/**
the proxy handler
*/
#[derive(Debug)]
pub struct Proxy<R: Connector, C: AsConnector<R> = ()> {
    target: Url,
    client: Client<<C as AsConnector<R>>::Connector>,
    pass_through_not_found: bool,
    halt: bool,
}

impl<R: Connector> Proxy<R, ()> {
    /**
    construct a new proxy handler that sends all requests to the url
    provided.  if the url contains a path, the inbound request path
    will be joined onto the end.

    ```
    let handler = trillium_proxy::Proxy::new("http://docs.trillium.rs/trillium_proxy");
    # drop(trillium_smol::run_async(handler)); // to infer the connector

    ```

     */
    pub fn new(target: impl TryInto<Url>) -> Self {
        let url = match target.try_into() {
            Ok(url) => url,
            Err(_) => panic!("could not convert proxy target into a url"),
        };

        if url.cannot_be_a_base() {
            panic!("{} cannot be a base", url);
        }

        Proxy {
            target: url,
            client: Client::new().with_default_pool(),
            pass_through_not_found: true,
            halt: true,
        }
    }
}

impl<R: Connector, C: AsConnector<R>> Proxy<R, C> {
    /**
    chainable constructor to specify the client Connector
    configuration

    ```
    use trillium_smol::ClientConfig;
    use trillium_proxy::Proxy;

    let proxy = Proxy::new("http://trillium.rs")
        .with_config(ClientConfig { //<-
            nodelay: Some(true),
            ..Default::default()
        });

    # drop(trillium_smol::run_async(proxy)); // in order to infer the connector
    ```
    */
    pub fn with_config(
        mut self,
        config: impl Into<<<C as AsConnector<R>>::Connector as Connector>::Config>,
    ) -> Self {
        self.client = self.client.with_config(config.into());
        self
    }

    /**
    use an additional connector that can be layered on top of the
    base connector.

    in practice, this currently means using
    [`trillium_native_tls::NativeTlsConnector::new`] or
    [`trillium_rustls::RustlsConnector::new`].

    **Note:** if your trillium application uses a tls acceptor to
    _serve_ tls, Proxy will detect that and use that tls
    implementation for outbound requests.

    ## examples

    ```
    use trillium_proxy::Proxy;
    use trillium_rustls::{RustlsConnector, rustls::ClientConfig};

    let handler = Proxy::new("https://www.google.com")
        .with_connector(RustlsConnector::new())
        .with_config(ClientConfig::new());

    # drop(trillium_smol::run_async(handler));
    ```


    ```
    use trillium_proxy::Proxy;
    use trillium_rustls::{RustlsConnector, RustlsConfig};
    use trillium_smol::ClientConfig;

    let handler = Proxy::new("https://www.google.com")
        .with_connector(RustlsConnector::new())
        .with_config(RustlsConfig {
            tcp_config: ClientConfig {
               nodelay: Some(true),
               ..Default::default()
            },
            ..Default::default()
        });

    # drop(trillium_smol::run_async(handler));
    ```
    */

    pub fn with_connector<NewConnector: AsConnector<R>>(
        self,
        connector: NewConnector,
    ) -> Proxy<R, NewConnector> {
        drop(connector);
        Proxy {
            target: self.target,
            client: Client::new().with_default_pool(),
            pass_through_not_found: self.pass_through_not_found,
            halt: self.halt,
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
    pub fn with_client(mut self, client: Client<<C as AsConnector<R>>::Connector>) -> Self {
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
impl<R: Connector, C: AsConnector<R>> Handler<R> for Proxy<R, C> {
    async fn run(&self, mut conn: Conn) -> Conn {
        let request_url = conn_try!(self.target.clone().join(conn.path()), conn);

        let mut client_conn = self.client.build_conn(conn.method(), request_url);

        for (name, value) in conn.headers() {
            for value in value {
                if name != "host" {
                    client_conn
                        .request_headers()
                        .insert(name.as_str(), value.as_str());
                }
            }
        }

        // need a better solution for streaming request bodies through
        // the proxy, but http-types::Body needs to be 'static. Fixing
        // this probably will entail moving away from http-types::Body
        // for outbound bodies.
        //
        // this is very inefficient and possibly unscalable in
        // situations where request bodies are large. there is no
        // reason that we couldn't have another lifetime on client
        // conn here, though
        let request_body = conn.request_body().await;

        if let Ok(client_body_content) = request_body.read_bytes().await {
            client_conn.set_request_body(client_body_content);
        }

        if client_conn
            .request_headers()
            .contains_ignore_ascii_case("connection", "close")
            || client_conn.request_headers().get("connection").is_none()
        {
            client_conn
                .request_headers()
                .insert("connection", "keep-alive");
        }

        trillium::conn_try!(client_conn.send().await, conn);

        let conn = match client_conn.status() {
            Some(SwitchingProtocols) => {
                for (name, value) in client_conn.response_headers() {
                    for value in value {
                        conn.headers_mut().append(name.as_str(), value.as_str());
                    }
                }

                conn.with_state(UpstreamUpgrade(client_conn.into()))
                    .with_status(SwitchingProtocols)
            }

            Some(NotFound) if self.pass_through_not_found => {
                client_conn.recycle().await;
                return conn;
            }

            Some(status) => {
                for (name, value) in client_conn.response_headers() {
                    for value in value {
                        conn.headers_mut().append(name.as_str(), value.as_str());
                    }
                }

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
            .get::<UpstreamUpgrade<R::Transport>>()
            .is_some()
    }

    async fn upgrade(&self, mut upgrade: trillium::Upgrade) {
        let upstream = upgrade
            .state
            .remove::<UpstreamUpgrade<R::Transport>>()
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
    Size::to_string(&Size::Bytes(bytes), Base::Base10, Style::Smart)
}
