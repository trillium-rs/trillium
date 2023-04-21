use crate::TestTransport;
use std::sync::Arc;
use url::Url;

/// a bridge between trillium servers and clients
#[derive(Debug)]
pub struct ServerConnector<H> {
    handler: Arc<H>,
}

impl<H> ServerConnector<H>
where
    H: trillium::Handler,
{
    /// builds a new ServerConnector
    pub fn new(handler: H) -> Self {
        Self {
            handler: Arc::new(handler),
        }
    }

    /// opens a new connection to this virtual server, returning the client transport
    pub async fn connect(&self, secure: bool) -> TestTransport {
        let (client_transport, server_transport) = TestTransport::new();

        let handler = Arc::clone(&self.handler);

        crate::spawn(async move {
            trillium_http::Conn::map(server_transport, Default::default(), |mut conn| {
                let handler = Arc::clone(&handler);
                async move {
                    conn.set_secure(secure);
                    let conn = handler.run(conn.into()).await;
                    let conn = handler.before_send(conn).await;
                    conn.into_inner()
                }
            })
            .await
            .unwrap();
        });

        client_transport
    }
}

#[trillium_server_common::async_trait]
impl<H: trillium::Handler> trillium_server_common::Connector for ServerConnector<H> {
    type Transport = TestTransport;
    async fn connect(&self, url: &Url) -> std::io::Result<Self::Transport> {
        Ok(self.connect(url.scheme() == "https").await)
    }

    fn spawn<Fut: std::future::Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        crate::spawn(fut)
    }
}

/// build a connector from this handler
pub fn connector(handler: impl trillium::Handler) -> impl trillium_server_common::Connector {
    ServerConnector::new(handler)
}

#[cfg(test)]
mod test {
    use crate::server_connector::ServerConnector;
    use trillium_client::Client;

    #[test]
    fn test() {
        crate::block_on(async {
            let client = Client::new(ServerConnector::new("test"));
            let mut conn = client.get("https://example.com/test").await.unwrap();
            assert_eq!(conn.response_body().read_string().await.unwrap(), "test");
        });
    }

    #[test]
    fn test_post() {
        crate::block_on(async {
            let client = Client::new(ServerConnector::new(
                |mut conn: trillium::Conn| async move {
                    let body = conn.request_body_string().await.unwrap();
                    let response = format!(
                        "{} {}://{}{} with body \"{}\"",
                        conn.method(),
                        if conn.is_secure() { "https" } else { "http" },
                        conn.inner().host().unwrap_or_default(),
                        conn.path(),
                        body
                    );

                    conn.ok(response)
                },
            ));

            let body = client
                .post("https://example.com/test")
                .with_body("some body")
                .await
                .unwrap()
                .response_body()
                .read_string()
                .await
                .unwrap();

            assert_eq!(
                body,
                "POST https://example.com/test with body \"some body\""
            );
        });
    }
}
