use crate::{CloneCounter, Server};
use myco::{BoxedTransport, Conn, Error, Handler, Transport};
use myco_http::Conn as HttpConn;
use myco_http::Stopper;
use myco_tls_common::Acceptor;
use std::convert::{TryFrom, TryInto};
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::net::{SocketAddr, TcpListener, ToSocketAddrs};

pub struct Config<S, A, T> {
    acceptor: A,
    port: Option<u16>,
    host: Option<String>,
    transport: PhantomData<T>,
    server: PhantomData<S>,
    nodelay: bool,
    stopper: Stopper,
    counter: CloneCounter,
    register_signals: bool,
}

impl<S, A: Clone, T> Clone for Config<S, A, T> {
    fn clone(&self) -> Self {
        Self {
            acceptor: self.acceptor.clone(),
            port: self.port,
            host: self.host.clone(),
            transport: PhantomData,
            server: PhantomData,
            nodelay: self.nodelay,
            stopper: self.stopper.clone(),
            counter: self.counter.clone(),
            register_signals: self.register_signals,
        }
    }
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
            stopper: Stopper::new(),
            counter: CloneCounter::new(),
            register_signals: cfg!(unix),
        }
    }
}

impl<S, T> Config<S, (), T> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S, A, T> Config<S, A, T>
where
    S: Server<Transport = T>,
    A: Acceptor<T>,
    T: Transport,
{
    pub fn acceptor(&self) -> &A {
        &self.acceptor
    }

    pub fn socket_addrs(&self) -> Vec<SocketAddr> {
        (self.host(), self.port())
            .to_socket_addrs()
            .unwrap()
            .collect()
    }

    pub fn build_listener<Listener>(&self) -> Listener
    where
        Listener: TryFrom<TcpListener>,
        <Listener as TryFrom<TcpListener>>::Error: std::fmt::Debug,
    {
        #[cfg(unix)]
        let listener = {
            use std::os::unix::prelude::FromRawFd;

            if let Some(fd) = std::env::var("LISTEN_FD")
                .ok()
                .and_then(|fd| fd.parse().ok())
            {
                log::debug!("using fd {} from LISTEN_FD", fd);
                unsafe { TcpListener::from_raw_fd(fd) }
            } else {
                TcpListener::bind((self.host(), self.port())).unwrap()
            }
        };

        #[cfg(not(unix))]
        let listener = TcpListener::bind((self.host(), self.port())).unwrap();

        log::info!("listening on {:?}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        listener.try_into().unwrap()
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

    pub fn counter(&self) -> &CloneCounter {
        &self.counter
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

    pub fn without_signals(mut self) -> Self {
        self.register_signals = false;
        self
    }

    pub fn should_register_signals(&self) -> bool {
        self.register_signals
    }

    pub fn set_nodelay(mut self) -> Self {
        self.nodelay = true;
        self
    }

    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    pub fn with_acceptor<A1: Acceptor<T>>(self, acceptor: A1) -> Config<S, A1, T> {
        Config {
            host: self.host,
            port: self.port,
            nodelay: self.nodelay,
            transport: PhantomData,
            server: PhantomData,
            stopper: self.stopper,
            acceptor,
            counter: self.counter,
            register_signals: self.register_signals,
        }
    }

    pub async fn graceful_shutdown(self) {
        let current = self.counter.current();
        if current > 0 {
            log::info!(
                "waiting for {} open connection{} to close",
                current,
                if current == 1 { "" } else { "s" }
            );
            self.counter.await;
            log::info!("all done!")
        }
    }

    pub async fn handle_stream(self, stream: T, handler: impl Handler) {
        let stream = match self.acceptor.accept(stream).await {
            Ok(stream) => stream,
            Err(e) => {
                log::error!("acceptor error: {:?}", e);
                return;
            }
        };

        let result = HttpConn::map(stream, self.stopper.clone(), |conn| async {
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

            Err(Error::Closed) | Ok(None) => {
                log::debug!("closing connection");
            }

            Err(Error::Io(e)) if e.kind() == ErrorKind::ConnectionReset => {
                log::debug!("closing connection");
            }

            Err(e) => {
                log::error!("http error: {:?}", e);
            }
        };
    }
}
