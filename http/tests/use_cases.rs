/// This file represents representative use cases in order to ensure future changes take them
/// into consideration
use std::{future::Future, marker::PhantomData, sync::Arc};
use test_harness::test;
use trillium_client::{Client, Connector, Url};
use trillium_http::{Conn, KnownHeaderName, ServerConfig};
use trillium_testing::{harness, Runtime, TestResult, TestTransport};

#[test(harness)]
async fn send_no_server_header() -> TestResult {
    let client = Client::new(ServerConnector::new(|mut conn| async move {
        conn.response_headers_mut().remove(KnownHeaderName::Server);
        conn
    }));
    let conn = client.get("http://_").await.unwrap();
    assert!(!conn.response_headers().has_header(KnownHeaderName::Server));
    Ok(())
}

#[derive(Debug)]
pub struct ServerConnector<F, Fut> {
    handler: Arc<F>,
    fut: PhantomData<Fut>,
    runtime: Runtime,
    server_config: Arc<ServerConfig>,
}

impl<F, Fut> ServerConnector<F, Fut>
where
    F: Fn(Conn<TestTransport>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn<TestTransport>> + Send + Sync + 'static,
{
    fn new(handler: F) -> Self {
        Self {
            handler: Arc::new(handler),
            fut: PhantomData,
            server_config: ServerConfig::default().into(),
            runtime: trillium_testing::runtime().into(),
        }
    }
}

impl<F, Fut> Connector for ServerConnector<F, Fut>
where
    F: Fn(Conn<TestTransport>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn<TestTransport>> + Send + Sync + 'static,
{
    type Runtime = Runtime;
    type Transport = TestTransport;

    async fn connect(&self, _: &Url) -> std::io::Result<Self::Transport> {
        let (client_transport, server_transport) = TestTransport::new();

        let handler = self.handler.clone();
        let server_config = self.server_config.clone();

        self.runtime
            .spawn(async move { server_config.run(server_transport, &*handler).await });

        Ok(client_transport)
    }

    fn runtime(&self) -> Self::Runtime {
        self.runtime.clone()
    }
}
