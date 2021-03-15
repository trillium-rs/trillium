use full_duplex_async_copy::full_duplex_copy;
use myco::http_types::StatusCode;
use myco::{async_trait, conn_try, BoxedTransport, Conn, Handler};
use myco_client::Client;
use myco_http::Upgrade;
use size::{Base, Size, Style};
use std::convert::TryInto;
use url::Url;
use StatusCode::{NotFound, SwitchingProtocols};

pub use async_net::TcpStream;
pub use myco_client::{ClientTransport, NativeTls, NativeTlsConfig, Rustls, RustlsConfig};

pub struct Proxy<Transport: ClientTransport> {
    target: Url,
    client: Client<Transport>,
    pass_through_not_found: bool,
}

struct UpstreamUpgrade<T>(Upgrade<T>);

#[async_trait]
impl<Transport: ClientTransport> Handler for Proxy<Transport> {
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

        myco::conn_try!(conn, client_conn.send().await);

        match client_conn.status() {
            Some(SwitchingProtocols) => {
                for (name, value) in client_conn.response_headers() {
                    for value in value {
                        conn.headers_mut().append(name.as_str(), value.as_str());
                    }
                }

                conn.with_state(UpstreamUpgrade(client_conn.into()))
                    .status(SwitchingProtocols)
                    .halt()
            }

            Some(NotFound) if self.pass_through_not_found => {
                client_conn.recycle().await;
                conn
            }

            Some(status) => {
                for (name, value) in client_conn.response_headers() {
                    for value in value {
                        conn.headers_mut().append(name.as_str(), value.as_str());
                    }
                }

                conn.body(client_conn).status(status).halt()
            }
            _ => unreachable!(),
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade<BoxedTransport>) -> bool {
        upgrade.state.get::<UpstreamUpgrade<Transport>>().is_some()
    }

    async fn upgrade(&self, mut upgrade: myco::Upgrade) {
        let upstream = upgrade
            .state
            .remove::<UpstreamUpgrade<Transport>>()
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

impl<Transport: ClientTransport> Proxy<Transport> {
    pub fn new(target: impl TryInto<Url>) -> Self {
        let url = match target.try_into() {
            Ok(url) => url,
            Err(_) => panic!("could not convert proxy target into a url"),
        };

        Self {
            target: url,
            client: Client::new().with_default_pool(),
            pass_through_not_found: true,
        }
    }
}
