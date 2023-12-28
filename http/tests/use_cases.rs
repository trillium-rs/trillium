/// This file represents representative use cases in order to ensure future changes take them into
/// consideration
use std::{future::Future, marker::PhantomData, sync::Arc};
use test_harness::test;
use trillium_client::{Client, Connector, Url};
use trillium_http::{Conn, KnownHeaderName};
use trillium_testing::{TestResult, TestTransport};

#[test(harness = trillium_testing::harness)]
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
        }
    }
}

#[trillium_client::async_trait]
impl<F, Fut> Connector for ServerConnector<F, Fut>
where
    F: Fn(Conn<TestTransport>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn<TestTransport>> + Send + Sync + 'static,
{
    type Transport = TestTransport;

    async fn connect(&self, _: &Url) -> std::io::Result<Self::Transport> {
        let (client_transport, server_transport) = TestTransport::new();

        let handler = self.handler.clone();

        trillium_testing::spawn(async move {
            Conn::map(server_transport, Default::default(), &*handler)
                .await
                .unwrap();
        });

        Ok(client_transport)
    }

    fn spawn<SpawnFut: Future<Output = ()> + Send + 'static>(&self, fut: SpawnFut) {
        trillium_testing::spawn(fut);
    }
}
