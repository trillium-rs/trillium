use crate::{RuntimeType, TestTransport};
use std::{io, sync::Arc};
use trillium::Handler;
use trillium_http::Conn;
use trillium_server_common::Connector;
use url::Url;

/// a bridge between trillium servers and clients
#[derive(Debug)]
pub struct ServerConnector<H> {
    handler: Arc<H>,
    runtime: RuntimeType,
}

impl<H: Handler> ServerConnector<H> {
    /// builds a new ServerConnector
    pub fn new(handler: H) -> Self {
        Self {
            handler: Arc::new(handler),
            runtime: RuntimeType::default(),
        }
    }

    /// opens a new connection to this virtual server, returning the client transport
    pub async fn connect(&self, secure: bool) -> TestTransport {
        let (client_transport, server_transport) = TestTransport::new();

        let handler = Arc::clone(&self.handler);

        self.runtime.spawn(async move {
            Conn::map(server_transport, Default::default(), |mut conn| {
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

impl<H: Handler> Connector for ServerConnector<H> {
    type Transport = TestTransport;
    type Runtime = RuntimeType;
    async fn connect(&self, url: &Url) -> io::Result<Self::Transport> {
        Ok(self.connect(url.scheme() == "https").await)
    }

    fn runtime(&self) -> Self::Runtime {
        #[allow(clippy::clone_on_copy)]
        self.runtime.clone()
    }
}

/// build a connector from this handler
pub fn connector(handler: impl Handler) -> impl Connector {
    ServerConnector::new(handler)
}
