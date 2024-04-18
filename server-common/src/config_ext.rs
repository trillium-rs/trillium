use crate::{Acceptor, Config, Server, Transport};
use futures_lite::prelude::*;
use std::{
    io::ErrorKind,
    net::{SocketAddr, TcpListener, ToSocketAddrs},
    sync::Arc,
};
use trillium::Handler;
use trillium_http::{transport::BoxedTransport, Error, Swansong, SERVICE_UNAVAILABLE};
/// # Server-implementer interfaces to Config
///
/// These functions are intended for use by authors of trillium servers,
/// and should not be necessary to build an application. Please open
/// an issue if you find yourself using this trait directly in an
/// application.

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

    /// returns a clone of the [`Swansong`] associated with
    /// this server, to be used in conjunction with signals or other
    /// service interruption methods
    fn swansong(&self) -> Swansong;

    /// returns the tls acceptor for this server
    fn acceptor(&self) -> &AcceptorType;

    /// waits for all requests to complete
    fn graceful_shutdown(&self) -> impl Future<Output = ()> + Send;

    /// apply the provided handler to the transport, using
    /// [`trillium_http`]'s http implementation. this is the default inner
    /// loop for most trillium servers
    fn handle_stream(
        self: Arc<Self>,
        stream: ServerType::Transport,
        handler: impl Handler,
    ) -> impl Future<Output = ()> + Send;

    /// builds any type that is TryFrom<std::net::TcpListener> and
    /// configures it for use. most trillium servers should use this if
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

    /// determines if the server is currently responding to more than
    /// the maximum number of connections set by
    /// `Config::with_max_connections`.
    fn over_capacity(&self) -> bool;
}

impl<ServerType, AcceptorType> ConfigExt<ServerType, AcceptorType>
    for Config<ServerType, AcceptorType>
where
    ServerType: Server + Send + ?Sized,
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

    fn swansong(&self) -> Swansong {
        self.swansong.clone()
    }

    fn acceptor(&self) -> &AcceptorType {
        &self.acceptor
    }

    async fn graceful_shutdown(&self) {
        self.swansong.shut_down().await
    }

    async fn handle_stream(
        self: Arc<Self>,
        mut stream: ServerType::Transport,
        handler: impl Handler,
    ) {
        if self.over_capacity() {
            let mut byte = [0u8]; // wait for the client to start requesting
            trillium::log_error!(stream.read(&mut byte).await);
            trillium::log_error!(stream.write_all(SERVICE_UNAVAILABLE).await);
            return;
        }

        let guard = self.swansong.guard();

        trillium::log_error!(stream.set_nodelay(self.nodelay));

        let peer_ip = stream.peer_addr().ok().flatten().map(|addr| addr.ip());

        let transport = match self.acceptor.accept(stream).await {
            Ok(stream) => stream,
            Err(e) => {
                log::error!("acceptor error: {:?}", e);
                return;
            }
        };

        let handler = &handler;
        let result = trillium_http::Conn::map_with_config(
            self.http_config,
            transport,
            self.swansong.clone(),
            |mut conn| async {
                conn.set_peer_ip(peer_ip);
                let conn = handler.run(conn.into()).await;
                let conn = handler.before_send(conn).await;

                conn.into_inner()
            },
        )
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

            Err(Error::Io(e))
                if e.kind() == ErrorKind::ConnectionReset || e.kind() == ErrorKind::BrokenPipe =>
            {
                log::debug!("closing connection");
            }

            Err(e) => {
                log::error!("http error: {:?}", e);
            }
        };

        drop(guard);
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

        listener.set_nonblocking(true).unwrap();
        listener.try_into().unwrap()
    }

    fn over_capacity(&self) -> bool {
        self.max_connections
            .map_or(false, |m| self.swansong.guard_count() >= m)
    }
}
