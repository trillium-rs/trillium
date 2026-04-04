/// This file represents representative use cases in order to ensure future changes take them
/// into consideration
use std::{future::Future, io, marker::PhantomData, net::SocketAddr, sync::Arc};
use test_harness::test;
use trillium_client::{Client, Connector, Url};
use trillium_http::{Conn, KnownHeaderName, HttpContext};
use trillium_testing::{Runtime, TestResult, TestTransport, harness};

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
    context: Arc<HttpContext>,
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
            context: HttpContext::default().into(),
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
    type Udp = ();

    async fn connect(&self, _: &Url) -> io::Result<Self::Transport> {
        let (client_transport, server_transport) = TestTransport::new();

        let handler = self.handler.clone();
        let context = self.context.clone();

        self.runtime
            .spawn(async move { context.run(server_transport, &*handler).await });

        Ok(client_transport)
    }

    fn runtime(&self) -> Self::Runtime {
        self.runtime.clone()
    }

    async fn resolve(&self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(vec![])
    }
}
