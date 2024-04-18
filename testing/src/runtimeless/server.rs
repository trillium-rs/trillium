use super::SERVERS;
use crate::{RuntimelessRuntime, TestTransport};
use async_channel::Receiver;
use std::io::{Error, ErrorKind, Result};
use trillium::Info;
use trillium_server_common::{Acceptor, Config, ConfigExt, Server};
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
    type Transport = TestTransport;
    type Runtime = RuntimelessRuntime;

    const DESCRIPTION: &'static str = "test server";

    fn runtime() -> Self::Runtime {
        RuntimelessRuntime::default()
    }

    async fn accept(&mut self) -> Result<Self::Transport> {
        self.channel
            .recv()
            .await
            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
    }

    fn build_listener<A>(config: &Config<Self, A>) -> Self
    where
        A: Acceptor<Self::Transport>,
    {
        let mut port = config.port();
        let host = config.host();
        if port == 0 {
            loop {
                port = fastrand::u16(..);
                if !SERVERS.contains_key(&(host.clone(), port)) {
                    break;
                }
            }
        }

        let entry = SERVERS
            .entry((host.clone(), port))
            .or_insert_with(async_channel::unbounded);

        let (_, channel) = entry.value();

        Self {
            host,
            channel: channel.clone(),
            port,
        }
    }

    async fn clean_up(self) {
        SERVERS.remove(&(self.host, self.port));
    }

    fn info(&self) -> Info {
        let mut info = Info::from(&*format!("{}:{}", &self.host, &self.port));
        info.state_mut()
            .insert(Url::parse(&format!("http://{}:{}", &self.host, self.port)).unwrap());
        info
    }
}
