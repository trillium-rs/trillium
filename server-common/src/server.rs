use crate::{Acceptor, Config, ConfigExt, Stopper, Transport};
use std::{
    future::{ready, Future},
    io::Result,
    pin::Pin,
    sync::Arc,
};
use trillium::{Handler, Info};

/**
The server trait, for standard network-based server implementations.
*/
pub trait Server: Sized + Send + Sync + 'static {
    /// the individual byte stream that http
    /// will be communicated over. This is often an async "stream"
    /// like TcpStream or UnixStream. See [`Transport`]
    type Transport: Transport;

    /// The description of this server, to be appended to the Info and potentially logged.
    const DESCRIPTION: &'static str;

    /// Asynchronously return a single `Self::Transport` from a
    /// `Self::Listener`. Must be implemented.
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Self::Transport>> + Send + '_>>;

    /// Build an [`Info`] from the Self::Listener type. See [`Info`]
    /// for more details.
    fn info(&self) -> Info;

    /// After the server has shut down, perform any housekeeping, eg
    /// unlinking a unix socket.
    fn clean_up(self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(ready(()))
    }

    /// Build a listener from the config. The default logic for this
    /// is described elsewhere. To override the default logic, server
    /// implementations could potentially implement this directly.  To
    /// use this default logic, implement
    /// [`Server::listener_from_tcp`] and
    /// [`Server::listener_from_unix`].
    #[cfg(unix)]
    fn build_listener<A>(config: &Config<Self, A>) -> Self
    where
        A: Acceptor<Self::Transport>,
    {
        if let Some(listener) = config.binding.write().unwrap().take() {
            log::debug!("taking prebound listener");
            return listener;
        }

        use std::os::unix::prelude::FromRawFd;
        let host = config.host();
        if host.starts_with(|c| c == '/' || c == '.' || c == '~') {
            Self::listener_from_unix(std::os::unix::net::UnixListener::bind(host).unwrap())
        } else {
            let tcp_listener = if let Some(fd) = std::env::var("LISTEN_FD")
                .ok()
                .and_then(|fd| fd.parse().ok())
            {
                log::debug!("using fd {} from LISTEN_FD", fd);
                unsafe { std::net::TcpListener::from_raw_fd(fd) }
            } else {
                std::net::TcpListener::bind((host, config.port())).unwrap()
            };

            tcp_listener.set_nonblocking(true).unwrap();
            Self::listener_from_tcp(tcp_listener)
        }
    }

    /// Build a listener from the config. The default logic for this
    /// is described elsewhere. To override the default logic, server
    /// implementations could potentially implement this directly.  To
    /// use this default logic, implement [`Server::listener_from_tcp`]
    #[cfg(not(unix))]
    fn build_listener<A>(config: &Config<Self, A>) -> Self
    where
        A: Acceptor<Self::Transport>,
    {
        if let Some(listener) = config.binding.write().unwrap().take() {
            log::debug!("taking prebound listener");
            return listener;
        }

        let tcp_listener = std::net::TcpListener::bind((config.host(), config.port())).unwrap();
        tcp_listener.set_nonblocking(true).unwrap();
        Self::listener_from_tcp(tcp_listener)
    }

    /// Build a Self::Listener from a tcp listener. This is called by
    /// the [`Server::build_listener`] default implementation, and
    /// is mandatory if the default implementation is used.
    fn listener_from_tcp(_tcp: std::net::TcpListener) -> Self {
        unimplemented!()
    }

    /// Build a Self::Listener from a tcp listener. This is called by
    /// the [`Server::build_listener`] default implementation. You
    /// will want to tag an implementation of this with #[cfg(unix)].
    #[cfg(unix)]
    fn listener_from_unix(_tcp: std::os::unix::net::UnixListener) -> Self {
        unimplemented!()
    }

    /// Implementation hook for listening for any os signals and
    /// stopping the provided [`Stopper`]. The returned future will be
    /// spawned using [`Server::spawn`]
    fn handle_signals(_stopper: Stopper) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(ready(()))
    }

    /// Runtime implementation hook for spawning a task.
    fn spawn(fut: impl Future<Output = ()> + Send + 'static);

    /// Runtime implementation hook for blocking on a top level future.
    fn block_on(fut: impl Future<Output = ()> + 'static);

    /// Run a trillium application from a sync context
    fn run<A, H>(config: Config<Self, A>, handler: H)
    where
        A: Acceptor<Self::Transport>,
        H: Handler,
    {
        Self::block_on(Self::run_async(config, handler))
    }

    /// Run a trillium application from an async context. The default
    /// implementation of this method contains the core logic of this
    /// Trait.
    fn run_async<A, H>(
        config: Config<Self, A>,
        mut handler: H,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>
    where
        A: Acceptor<Self::Transport>,
        H: Handler,
    {
        Box::pin(async move {
            if config.should_register_signals() {
                #[cfg(unix)]
                Self::spawn(Self::handle_signals(config.stopper()));

                #[cfg(not(unix))]
                log::error!("signals handling not supported on windows yet");
            }

            let mut listener = Self::build_listener(&config);
            let mut info = Self::info(&listener);
            info.server_description_mut().push_str(Self::DESCRIPTION);
            handler.init(&mut info).await;
            config.info.set(info);
            let config = Arc::new(config);
            let handler = Arc::new(handler);

            while let Some(stream) = config
                .stopper
                .stop_future(Self::accept(&mut listener))
                .await
            {
                match stream {
                    Ok(stream) => {
                        let config = Arc::clone(&config);
                        let handler = Arc::clone(&handler);
                        Self::spawn(async move { config.handle_stream(stream, handler).await })
                    }
                    Err(e) => log::error!("tcp error: {}", e),
                }
            }

            config.graceful_shutdown().await;
            Self::clean_up(listener).await;
        })
    }
}
