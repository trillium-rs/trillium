use async_net::TcpListener;
use futures_lite::StreamExt;
use std::{future::Future, sync::Arc};
use trillium_http::{
    Conn, HttpContext, Upgrade,
    h2::{H2Connection, H2Transport},
};
use trillium_testing::{Runtime, runtime};

/// E2e test fixture that runs trillium-http's HTTP/2 driver in h2c (cleartext) prior-
/// knowledge mode on a real TCP listener bound to `127.0.0.1:0`. Bypasses
/// `trillium::Handler` and `trillium-server-common`; tests provide a
/// `Fn(Conn<H2Transport>) -> Fut` directly, plus an optional upgrade closure for tests
/// that exercise the `Conn::upgrade()` → `Upgrade` path.
///
/// Clients reach this fixture by setting `with_http_version(Version::Http2)` on a
/// `trillium-client` request against the fixture's `http://` base URL — that triggers
/// h2c prior-knowledge dispatch (client writes the 24-byte preface up front; the h2
/// driver consumes it). The fixture does not negotiate via ALPN.
#[derive(Debug)]
pub struct H2cServer {
    base_url: String,
    context: Arc<HttpContext>,
}

impl H2cServer {
    pub async fn new<H, HFut>(handler: H) -> Self
    where
        H: Fn(Conn<H2Transport>) -> HFut + Send + Sync + 'static,
        HFut: Future<Output = Conn<H2Transport>> + Send + 'static,
    {
        Self::with_upgrade(handler, noop_upgrade).await
    }

    pub async fn with_upgrade<H, HFut, U, UFut>(handler: H, upgrade_handler: U) -> Self
    where
        H: Fn(Conn<H2Transport>) -> HFut + Send + Sync + 'static,
        HFut: Future<Output = Conn<H2Transport>> + Send + 'static,
        U: Fn(Upgrade<H2Transport>) -> UFut + Send + Sync + 'static,
        UFut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}/");
        let context = Arc::new(HttpContext::new());
        let handler = Arc::new(handler);
        let upgrade_handler = Arc::new(upgrade_handler);
        let rt: Runtime = runtime().into();

        let swansong = context.swansong().clone();
        let context_for_loop = context.clone();
        let rt_for_loop = rt.clone();

        rt.spawn(async move {
            let mut incoming = listener.incoming();
            while let Some(maybe_stream) = swansong.interrupt(incoming.next()).await {
                let Some(Ok(stream)) = maybe_stream else {
                    continue;
                };
                let context = context_for_loop.clone();
                let handler = handler.clone();
                let upgrade_handler = upgrade_handler.clone();
                let rt_inner = rt_for_loop.clone();
                rt_for_loop.spawn(async move {
                    let h2 = H2Connection::new(context);
                    let mut driver = h2.run(stream);
                    while let Some(result) = driver.next().await {
                        let Ok(conn) = result else { break };
                        let handler = handler.clone();
                        let upgrade_handler = upgrade_handler.clone();
                        rt_inner.spawn(async move {
                            let handler_for_run = handler.clone();
                            let result = H2Connection::process_inbound(conn, move |conn| {
                                let h = handler_for_run.clone();
                                async move { h(conn).await }
                            })
                            .await;

                            if let Ok(conn) = result
                                && conn.should_upgrade()
                            {
                                upgrade_handler(Upgrade::from(conn)).await;
                            }
                        });
                    }
                });
            }
        });

        Self { base_url, context }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn shut_down(self) {
        self.context.shut_down().await;
    }
}

async fn noop_upgrade<T>(_: Upgrade<T>) {}
