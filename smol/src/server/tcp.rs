use crate::{SmolRuntime, SmolTransport};
use async_net::{TcpListener, TcpStream};
use std::{convert::TryInto, env, io::Result, net};
use trillium::Info;
use trillium_server_common::{Server, Url};

#[derive(Debug)]
pub struct SmolTcpServer(TcpListener);
impl From<TcpListener> for SmolTcpServer {
    fn from(value: TcpListener) -> Self {
        Self(value)
    }
}

impl Server for SmolTcpServer {
    type Transport = SmolTransport<TcpStream>;
    type Runtime = SmolRuntime;

    const DESCRIPTION: &'static str = concat!(
        " (",
        env!("CARGO_PKG_NAME"),
        " v",
        env!("CARGO_PKG_VERSION"),
        ")"
    );

    async fn accept(&mut self) -> Result<Self::Transport> {
        self.0.accept().await.map(|(t, _)| t.into())
    }

    fn listener_from_tcp(tcp: net::TcpListener) -> Self {
        Self(tcp.try_into().unwrap())
    }

    fn info(&self) -> Info {
        let local_addr = self.0.local_addr().unwrap();
        let mut info = Info::from(local_addr);
        if let Ok(url) = Url::parse(&format!("http://{local_addr}")) {
            info.state_mut().insert(url);
        }
        info
    }

    fn runtime() -> Self::Runtime {
        SmolRuntime::default()
    }
}
