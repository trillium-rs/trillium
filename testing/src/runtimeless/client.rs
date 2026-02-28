use super::SERVERS;
use crate::{RuntimelessRuntime, TestTransport};
use std::io::{Error, ErrorKind, Result};
use trillium_server_common::Connector;
use url::Url;

/// An in-memory Connector to use with RuntimelessServer.
#[derive(Default, Debug, Clone, Copy)]
pub struct RuntimelessClientConfig(());

impl RuntimelessClientConfig {
    /// constructs a RuntimelessClientConfig
    pub fn new() -> Self {
        Self(())
    }
}

impl Connector for RuntimelessClientConfig {
    type Runtime = RuntimelessRuntime;
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

    fn runtime(&self) -> Self::Runtime {
        RuntimelessRuntime::default()
    }
}
