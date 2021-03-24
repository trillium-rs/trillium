use crate::{CloneCounter, Config, Server};
use myco::{BoxedTransport, Conn, Handler};
use myco_http::{Conn as HttpConn, Error, Stopper};
use myco_tls_common::Acceptor;
use std::{
    convert::{TryFrom, TryInto},
    io::ErrorKind,
    net::{SocketAddr, TcpListener, ToSocketAddrs},
};
/// # Server-implementer interfaces to Config
///
/// These functions are intended for use by authors of myco servers,
/// and should not be necessary to build an application. Please open
/// an issue if you find yourself using this trait directly in an
/// application.

#[myco::async_trait]
pub trait ConfigExt<ServerType, AcceptorType>
where
    ServerType: Server,
{
    /// resolve a port for this application, either directly
    /// configured, from the environmental variable `PORT`, or a default
    /// of `8080`
    fn port(&self) -> u16;

    /// resolve the host for this application, either directly from
    /// configuration, from the `HOST` env var, or `"localhost"`
    fn host(&self) -> String;

    /// use the [`ConfigExt::port`] and [`ConfigExt::host`] to resolve
    /// a vec of potential socket addrs
    fn socket_addrs(&self) -> Vec<SocketAddr>;

    /// returns whether this server should register itself for
    /// operating system signals. this flag does nothing aside from
    /// communicating to the server implementer that this is
    /// desired. defaults to true on `cfg(unix)` systems, and false
    /// elsewhere.
    fn should_register_signals(&self) -> bool;

    /// returns whether the server should set TCP_NODELAY on the
    /// TcpListener, if that is applicable
    fn nodelay(&self) -> bool;

    /// returns a clone of the [`Stopper`] associated with
    /// this server, to be used in conjunction with signals or other
    /// service interruption methods
    fn stopper(&self) -> Stopper;

    /// returns the tls acceptor for this server
    fn acceptor(&self) -> &AcceptorType;

    /// returns the [`CloneCounter`] for this server. please note that
    /// cloning this type has implications for graceful shutdown and
    /// needs to be done with care.
    fn counter(&self) -> &CloneCounter;

    /// waits for the last clone of the [`CloneCounter`] in this
    /// config to drop, indicating that all outstanding requests are
    /// complete
    async fn graceful_shutdown(self);

    /// apply the provided handler to the transport, using
    /// [`myco_http`]'s http implementation. this is the default inner
    /// loop for most myco servers
    async fn handle_stream(self, stream: ServerType::Transport, handler: impl Handler);

    /// builds any type that is TryFrom<std::net::TcpListener> and
    /// configures it for use. most myco servers should use this if
    /// possible instead of using [`ConfigExt::port`],
    /// [`ConfigExt::host`], or [`ConfigExt::socket_addrs`].
    ///
    /// this function also contains logic that sets nonblocking to
    /// true and on unix systems will build a tcp listener from the
    /// `LISTEN_FD` env var.
    fn build_listener<Listener>(&self) -> Listener
    where
        Listener: TryFrom<TcpListener>,
        <Listener as TryFrom<TcpListener>>::Error: std::fmt::Debug;
}

#[myco::async_trait]
impl<ServerType, AcceptorType> ConfigExt<ServerType, AcceptorType>
    for Config<ServerType, AcceptorType>
where
    ServerType: Server + Send,
    AcceptorType: Acceptor<<ServerType as Server>::Transport>,
{
    fn port(&self) -> u16 {
        self.port
            .or_else(|| std::env::var("PORT").ok().and_then(|p| p.parse().ok()))
            .unwrap_or(8080)
    }

    fn host(&self) -> String {
        self.host
            .as_ref()
            .map(String::from)
            .or_else(|| std::env::var("HOST").ok())
            .unwrap_or_else(|| String::from("localhost"))
    }

    fn socket_addrs(&self) -> Vec<SocketAddr> {
        (self.host(), self.port())
            .to_socket_addrs()
            .unwrap()
            .collect()
    }

    fn should_register_signals(&self) -> bool {
        self.register_signals
    }

    fn nodelay(&self) -> bool {
        self.nodelay
    }

    fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    fn acceptor(&self) -> &AcceptorType {
        &self.acceptor
    }

    fn counter(&self) -> &CloneCounter {
        &self.counter
    }

    async fn graceful_shutdown(self) {
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

    async fn handle_stream(self, stream: ServerType::Transport, handler: impl Handler) {
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

    fn build_listener<Listener>(&self) -> Listener
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
}
