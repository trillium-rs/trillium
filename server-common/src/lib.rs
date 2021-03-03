use std::marker::PhantomData;
use std::net::ToSocketAddrs;

use myco::{async_trait, BoxedTransport, Conn, Handler, Transport};
use myco_http::Conn as HttpConn;
pub use myco_tls_common::Acceptor;

pub async fn handle_stream<T: Transport>(
    stream: T,
    acceptor: impl Acceptor<T>,
    handler: impl Handler,
) {
    let stream = match acceptor.accept(stream).await {
        Ok(stream) => stream,
        Err(e) => {
            log::error!("acceptor error: {:?}", e);
            return;
        }
    };

    let result = HttpConn::map(stream, |conn| async {
        let conn = Conn::new(conn);
        let conn = handler.run(conn).await;
        let conn = handler.before_send(conn).await;
        conn.into_inner()
    })
    .await;

    match result {
        Ok(Some(upgrade)) => {
            let upgrade = upgrade.map_transport(BoxedTransport::new);
            if handler.has_upgrade(&upgrade) {
                log::debug!("upgrading...");
                handler.upgrade(upgrade).await;
            } else {
                log::error!("upgrade specified but no upgrade handler provided");
            }
        }

        Ok(None) => {
            log::debug!("closing connection");
        }

        Err(e) => {
            log::error!("http error: {:?}", e);
        }
    };
}

pub struct Config<S, A, T> {
    acceptor: A,
    port: Option<u16>,
    host: Option<String>,
    transport: PhantomData<T>,
    server: PhantomData<S>,
    nodelay: bool,
}

impl<S, T> Default for Config<S, (), T> {
    fn default() -> Self {
        Self {
            acceptor: (),
            port: None,
            host: None,
            transport: PhantomData,
            server: PhantomData,
            nodelay: false,
        }
    }
}

impl<S, T> Config<S, (), T> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S: Server<Transport = T>, A: Acceptor<T>, T: Transport> Config<S, A, T> {
    pub fn acceptor(&self) -> &A {
        &self.acceptor
    }

    pub fn socket_addrs(&self) -> Vec<std::net::SocketAddr> {
        (self.host(), self.port())
            .to_socket_addrs()
            .unwrap()
            .collect()
    }

    pub fn host(&self) -> String {
        self.host
            .as_ref()
            .map(String::from)
            .or_else(|| std::env::var("HOST").ok())
            .unwrap_or_else(|| String::from("localhost"))
    }

    pub fn port(&self) -> u16 {
        self.port
            .or_else(|| std::env::var("PORT").ok().and_then(|p| p.parse().ok()))
            .unwrap_or(8080)
    }

    pub fn run<H: Handler>(self, h: H) {
        S::run(self, h)
    }

    pub async fn run_async(self, handler: impl Handler) {
        S::run_async(self, handler).await
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn with_host(mut self, host: &str) -> Self {
        self.host = Some(host.into());
        self
    }

    pub fn nodelay(&self) -> bool {
        self.nodelay
    }

    pub fn set_nodelay(mut self) -> Self {
        self.nodelay = true;
        self
    }

    pub fn with_acceptor<A1: Acceptor<T>>(self, acceptor: A1) -> Config<S, A1, T> {
        Config {
            host: self.host,
            port: self.port,
            nodelay: self.nodelay,
            transport: PhantomData,
            server: PhantomData,
            acceptor,
        }
    }
}

#[async_trait]
pub trait Server: Sized {
    type Transport: Transport;
    fn run<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<Self, A, Self::Transport>,
        handler: H,
    );

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<Self, A, Self::Transport>,
        handler: H,
    );
}
