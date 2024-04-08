use super::SERVERS;
use crate::{RuntimelessRuntime, TestTransport};
use async_channel::Receiver;
use std::io::{Error, ErrorKind, Result};
use trillium::Info;
use trillium_server_common::Server;
use url::Url;

/// A [`Server`] for testing that does not depend on any runtime
#[derive(Debug)]
pub struct RuntimelessServer {
    host: String,
    port: u16,
    channel: Receiver<TestTransport>,
}

impl RuntimelessServer {
    /// returns whether there are any currently registered servers
    pub fn is_empty() -> bool {
        SERVERS.is_empty()
    }

    /// returns the number of currently registered servers
    pub fn len() -> usize {
        SERVERS.len()
    }
}

impl Server for RuntimelessServer {
    type Runtime = RuntimelessRuntime;
    type Transport = TestTransport;

    fn runtime() -> Self::Runtime {
        RuntimelessRuntime::default()
    }

    async fn accept(&mut self) -> Result<Self::Transport> {
        self.channel
            .recv()
            .await
            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
    }

    fn from_host_and_port(host: &str, mut port: u16) -> Self {
        if port == 0 {
            loop {
                port = fastrand::u16(..);
                if !SERVERS.contains_key(&(host.to_string(), port)) {
                    break;
                }
            }
        }

        let entry = SERVERS
            .entry((host.to_string(), port))
            .or_insert_with(async_channel::unbounded);

        let (_, channel) = entry.value();

        Self {
            host: host.to_string(),
            channel: channel.clone(),
            port,
        }
    }

    async fn clean_up(self) {
        SERVERS.remove(&(self.host, self.port));
    }

    fn init(&self, info: &mut Info) {
        info.insert_state(Url::parse(&format!("http://{}:{}", &self.host, self.port)).unwrap());
    }
}
