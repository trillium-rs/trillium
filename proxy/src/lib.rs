use full_duplex_async_copy::full_duplex_copy;
use size::{Base, Size, Style};
use std::convert::TryInto;
use trillium::http_types::StatusCode;
use trillium::{async_trait, conn_try, Conn, Handler};
use trillium_client::Client;
use trillium_http::{transport::BoxedTransport, Upgrade};
use url::Url;
use StatusCode::{NotFound, SwitchingProtocols};

pub use async_net::TcpStream;
pub use trillium_client::Connector;

pub struct Proxy<C: Connector> {
    target: Url,
    client: Client<C>,
    pass_through_not_found: bool,
    halt: bool,
}

struct UpstreamUpgrade<T>(Upgrade<T>);

#[async_trait]
impl<C: Connector> Handler for Proxy<C> {
    async fn run(&self, mut conn: Conn) -> Conn {
        let request_url = conn_try!(conn, self.target.clone().join(conn.path()));

        let mut client_conn = self.client.conn(*conn.method(), request_url);

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
        if let Ok(client_body_content) = conn.request_body().await.read_bytes().await {
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

        trillium::conn_try!(conn, client_conn.send().await);

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
            .get::<UpstreamUpgrade<C::Transport>>()
            .is_some()
    }

    async fn upgrade(&self, mut upgrade: trillium::Upgrade) {
        let upstream = upgrade
            .state
            .remove::<UpstreamUpgrade<C::Transport>>()
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

impl<C: Connector> Proxy<C> {
    pub fn with_config(mut self, config: C::Config) -> Self {
        self.client = self.client.with_config(config);
        self
    }

    pub fn with_client(mut self, client: Client<C>) -> Self {
        self.client = client;
        self
    }

    pub fn new(target: impl TryInto<Url>) -> Self {
        let url = match target.try_into() {
            Ok(url) => url,
            Err(_) => panic!("could not convert proxy target into a url"),
        };

        if url.cannot_be_a_base() {
            panic!("{} cannot be a base", url);
        }

        Self {
            target: url,
            client: Client::new().with_default_pool(),
            pass_through_not_found: true,
            halt: false,
        }
    }
}
