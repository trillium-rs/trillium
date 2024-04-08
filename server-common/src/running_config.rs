use crate::{Acceptor, ArcHandler, RuntimeTrait, Server};
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use std::{io::ErrorKind, sync::Arc};
use trillium::Handler;
use trillium_http::{
    transport::{BoxedTransport, Transport},
    Error, ServerConfig, SERVICE_UNAVAILABLE,
};

#[derive(Debug)]
pub struct RunningConfig<ServerType: Server, AcceptorType> {
    pub(crate) acceptor: AcceptorType,
    pub(crate) max_connections: Option<usize>,
    pub(crate) nodelay: bool,
    pub(crate) runtime: ServerType::Runtime,
    pub(crate) server_config: Arc<ServerConfig>,
}

impl<S: Server, A: Acceptor<<S as Server>::Transport>> RunningConfig<S, A> {
    pub(crate) async fn run_async(self: Arc<Self>, mut listener: S, handler: impl Handler) {
        let swansong = self.server_config.as_ref().swansong();
        let runtime = self.runtime.clone();
        let handler = ArcHandler::new(handler);
        while let Some(transport) = swansong.interrupt(listener.accept()).await {
            match transport {
                Ok(stream) => {
                    runtime.spawn(
                        Arc::clone(&self).handle_stream(stream, ArcHandler::clone(&handler)),
                    );
                }
                Err(e) => log::error!("tcp error: {}", e),
            }
        }

        self.server_config.swansong().shut_down().await;
        listener.clean_up().await;
    }

    async fn handle_stream(self: Arc<Self>, mut stream: S::Transport, handler: impl Handler) {
        if self.over_capacity() {
            let mut byte = [0u8]; // wait for the client to start requesting
            trillium::log_error!(stream.read(&mut byte).await);
            trillium::log_error!(stream.write_all(SERVICE_UNAVAILABLE).await);
            return;
        }

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

        let result = self
            .server_config
            .clone()
            .run(transport, |mut conn| async {
                conn.set_peer_ip(peer_ip);
                let conn = handler.run(conn.into()).await;
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

            Err(Error::Io(e))
                if e.kind() == ErrorKind::ConnectionReset || e.kind() == ErrorKind::BrokenPipe =>
            {
                log::debug!("closing connection");
            }

            Err(e) => {
                log::error!("http error: {:?}", e);
            }
        };
    }

    // fn build_listener<Listener>(&self) -> Listener
    // where
    //     Listener: TryFrom<TcpListener>,
    //     <Listener as TryFrom<TcpListener>>::Error: std::fmt::Debug,
    // {
    //     #[cfg(unix)]
    //     let listener = {
    //         use std::os::unix::prelude::FromRawFd;

    //         if let Some(fd) = std::env::var("LISTEN_FD")
    //             .ok()
    //             .and_then(|fd| fd.parse().ok())
    //         {
    //             log::debug!("using fd {} from LISTEN_FD", fd);
    //             unsafe { TcpListener::from_raw_fd(fd) }
    //         } else {
    //             TcpListener::bind((self.host(), self.port())).unwrap()
    //         }
    //     };

    //     #[cfg(not(unix))]
    //     let listener = TcpListener::bind((self.host(), self.port())).unwrap();

    //     listener.set_nonblocking(true).unwrap();
    //     listener.try_into().unwrap()
    // }

    fn over_capacity(&self) -> bool {
        self.max_connections
            .map_or(false, |m| self.server_config.swansong().guard_count() >= m)
    }
}
