use crate::{spawn, TestTransport};
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::{
    future::Future,
    io::{Error, ErrorKind, Result},
    pin::Pin,
};
use trillium::Info;
use trillium_server_common::{Acceptor, Config, ConfigExt, Connector, Server};
use url::Url;

static SERVERS: Lazy<DashMap<(String, u16), (Sender<TestTransport>, Receiver<TestTransport>)>> =
    Lazy::new(|| Default::default());

/// A [`Server`] for testing that does not depend on any runtime
#[derive(Debug)]
pub struct RuntimelessServer {
    host: String,
    port: u16,
    channel: Receiver<TestTransport>,
}

impl Server for RuntimelessServer {
    type Transport = TestTransport;
    const DESCRIPTION: &'static str = "test server";
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>> {
        Box::pin(async move {
            self.channel
                .recv()
                .await
                .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
        })
    }

    fn build_listener<A>(config: &Config<Self, A>) -> Self
    where
        A: Acceptor<Self::Transport>,
    {
        let entry = SERVERS
            .entry((config.host(), config.port()))
            .or_insert_with(|| async_channel::unbounded());
        let (_, channel) = entry.value();

        Self {
            host: config.host(),
            channel: channel.clone(),
            port: config.port(),
        }
    }

    fn info(&self) -> Info {
        Info::from(&*format!("test server :{}", self.port))
    }

    fn block_on(fut: impl Future<Output = ()> + 'static) {
        crate::block_on(fut)
    }

    fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        spawn(fut)
    }
}

impl Drop for RuntimelessServer {
    fn drop(&mut self) {
        SERVERS.remove(&(self.host.clone(), self.port));
    }
}

/// An in-memory Connector to use with GenericServer.
#[derive(Default, Debug, Clone, Copy)]
pub struct RuntimelessClientConfig;

impl RuntimelessClientConfig {
    /// constructs a GenericClientConfig
    pub fn new() -> Self {
        Self
    }
}

#[trillium::async_trait]
impl Connector for RuntimelessClientConfig {
    type Transport = TestTransport;
    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        let (tx, _) = &*SERVERS
            .get(&(
                url.host_str().unwrap().to_string(),
                url.port_or_known_default().unwrap(),
            ))
            .ok_or(Error::new(ErrorKind::AddrNotAvailable, "not available"))?;
        let (client_transport, server_transport) = TestTransport::new();
        tx.send(server_transport).await.unwrap();
        Ok(client_transport)
    }

    fn spawn<Fut: Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        spawn(fut)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{harness, TestResult};
    use test_harness::test;
    #[test(harness = harness)]
    async fn round_trip() -> TestResult {
        let handle1 = Config::<RuntimelessServer, ()>::new()
            .with_host("host.com")
            .with_port(80)
            .spawn("server 1");

        let handle2 = Config::<RuntimelessServer, ()>::new()
            .with_host("other_host.com")
            .with_port(80)
            .spawn("server 2");

        let client = trillium_client::Client::new(RuntimelessClientConfig::default());
        let mut conn = client.get("http://host.com").await?;
        assert_eq!(conn.response_body().await?, "server 1");

        let mut conn = client.get("http://other_host.com").await?;
        assert_eq!(conn.response_body().await?, "server 2");

        handle1.stop().await;
        assert!(client.get("http://host.com").await.is_err());
        assert!(client.get("http://other_host.com").await.is_ok());

        handle2.stop().await;
        assert!(client.get("http://other_host.com").await.is_err());

        assert!(SERVERS.is_empty());

        Ok(())
    }
}
