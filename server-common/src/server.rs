use trillium::Handler;

/**
The server trait, for standard network-based server implementations.
 */
pub trait Server: Send + Sized {
    /// run the given handler on this server
    #[must_use = "futures must be awaited"]
    fn run_async(
        self,
        handler: impl Handler<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

    /// server-specific implementation of signals handling
    fn handle_signals(&self) {}
}

/**
macro to build a server sharing the same implementation as
trillium-smol, trillium-async-std, and trillium-tokio
*/
#[macro_export]
macro_rules! standard_server {
    ($server:ident, transport: $transport:ty, listener: $listener:ty) => {
        use std::convert::{TryFrom, TryInto};

        impl<A> $server<A>
        where
            A: $crate::Acceptor<$transport>,
        {
            /// Configures a tls [`Acceptor`] for this server
            pub fn with_acceptor<B>(self, acceptor: B) -> $server<B>
            where
                B: Acceptor<$transport>,
            {
                $server {
                    config: self.config,
                    acceptor,
                }
            }

            /// Configures the server to listen on this port. The default is
            /// the `PORT` environment variable or 8080
            pub fn with_port(mut self, port: u16) -> Self {
                self.config.set_port(port);
                self
            }

            /// Configures the server to listen on this host or ip
            /// address. The default is the HOST environment variable or
            /// "localhost"
            pub fn with_host(mut self, host: &str) -> Self {
                self.config.set_host(host);
                self
            }

            /// Configures the server to NOT register for graceful-shutdown
            /// signals with the operating system. Default behavior is for the
            /// server to listen for `SIGINT and `SIGTERM` and perform a graceful
            /// shutdown.
            pub fn without_signals(mut self) -> Self {
                self.config.set_register_signals(false);
                self
            }

            /// Configures the tcp listener to use TCP_NODELAY. See
            /// <https://en.wikipedia.org/wiki/Nagle%27s_algorithm> for more
            /// information on this setting.
            pub fn with_nodelay(mut self) -> Self {
                self.config.set_nodelay(true);
                self
            }

            /// use the specific [`Stopper`] provided, replacing the
            /// default one
            pub fn with_stopper(mut self, stopper: Stopper) -> Self {
                self.config.set_stopper(stopper);
                self
            }

            fn port(&self) -> u16 {
                self.config
                    .port()
                    .or_else(|| std::env::var("PORT").ok().and_then(|p| p.parse().ok()))
                    .unwrap_or(8080)
            }

            fn host(&self) -> String {
                self.config
                    .host()
                    .map(String::from)
                    .or_else(|| std::env::var("HOST").ok())
                    .unwrap_or_else(|| String::from("localhost"))
            }

            fn should_register_signals(&self) -> bool {
                self.config.register_signals()
            }

            fn nodelay(&self) -> bool {
                self.config.nodelay()
            }

            fn stopper(&self) -> Stopper {
                self.config.stopper().clone()
            }

            async fn graceful_shutdown(self) {
                let current = self.config.counter().current();
                if current > 0 {
                    log::info!(
                        "waiting for {} open connection{} to close",
                        current,
                        if current == 1 { "" } else { "s" }
                    );
                    self.config.counter().await;
                    log::info!("all done!")
                }
            }

            async fn handle_stream(self, stream: $transport, handler: impl Handler<Self>) {
                let peer_ip = Self::peer_ip(&stream);

                let stream = match self.acceptor.accept(stream).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        log::error!("acceptor error: {:?}", e);
                        return;
                    }
                };

                let result =
                    trillium_http::Conn::map(stream, self.stopper().clone(), |mut conn| async {
                        conn.set_peer_ip(peer_ip);
                        let conn = handler.run(conn.into()).await;
                        let conn = handler.before_send(conn).await;

                        conn.into_inner()
                    })
                    .await;

                match result {
                    Ok(Some(upgrade)) => {
                        let upgrade =
                            upgrade.map_transport(trillium_http::transport::BoxedTransport::new);
                        if handler.has_upgrade(&upgrade) {
                            log::debug!("upgrading...");
                            handler.upgrade(upgrade).await;
                        } else {
                            log::error!("upgrade specified but no upgrade handler provided");
                        }
                    }

                    Err(trillium_http::Error::Closed) | Ok(None) => {
                        log::debug!("closing connection");
                    }

                    Err(trillium_http::Error::Io(e))
                        if e.kind() == std::io::ErrorKind::ConnectionReset =>
                    {
                        log::debug!("closing connection");
                    }

                    Err(e) => {
                        log::error!("http error: {:?}", e);
                    }
                };
            }

            fn build_listener(&self) -> $listener {
                #[cfg(unix)]
                let listener = {
                    use std::os::unix::prelude::FromRawFd;

                    if let Some(fd) = std::env::var("LISTEN_FD")
                        .ok()
                        .and_then(|fd| fd.parse().ok())
                    {
                        log::debug!("using fd {} from LISTEN_FD", fd);
                        unsafe { std::net::TcpListener::from_raw_fd(fd) }
                    } else {
                        std::net::TcpListener::bind((self.host(), self.port())).unwrap()
                    }
                };

                #[cfg(not(unix))]
                let listener = TcpListener::bind((self.host(), self.port())).unwrap();

                listener.set_nonblocking(true).unwrap();
                listener.try_into().unwrap()
            }

            /**
            entrypoint to run a handler with a given config. if this function
            is called, it is safe to assume that we are not yet within an
            async runtime's block_on. generally this should just entail a call
            to `my_runtime::block_on(Self::run_async(config, handler))`
            */
            pub fn run(self, handler: impl Handler<Self>) {
                Self::block_on(self.run_async(handler));
            }

            /**
            entrypoint to run a handler with a given config within a
            preexisting runtime
            */
            pub async fn run_async(self, mut handler: impl Handler<Self>) {
                if self.should_register_signals() {
                    self.handle_signals();
                }

                let listener = self.build_listener();
                let local_addr = listener.local_addr().unwrap();

                let mut incoming = self.stopper().stop_stream(listener.incoming());
                let mut info = Info::from(local_addr);
                *info.listener_description_mut() =
                    format!("http://{}:{}", self.host(), self.port());
                info.server_description_mut().push_str(SERVER_DESCRIPTION);

                handler.init(&mut info).await;
                let handler = Arc::new(handler);

                while let Some(Ok(stream)) = incoming.next().await {
                    trillium::log_error!(stream.set_nodelay(self.nodelay()));
                    Self::spawn(self.clone().handle_stream(stream, handler.clone()));
                }

                self.graceful_shutdown().await;
            }

            fn peer_ip(transport: &$transport) -> Option<IpAddr> {
                transport
                    .peer_addr()
                    .ok()
                    .map(|socket_addr| socket_addr.ip())
            }
        }
    };
}
