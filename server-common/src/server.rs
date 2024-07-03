use crate::{RuntimeTrait, Swansong, Transport};
use listenfd::ListenFd;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::{future::Future, io::Result, net::TcpListener};
use trillium::Info;

/// The server trait, for standard network-based server implementations.
pub trait Server: Sized + Send + Sync + 'static {
    /// the individual byte stream that http
    /// will be communicated over. This is often an async "stream"
    /// like TcpStream or UnixStream. See [`Transport`]
    type Transport: Transport;

    /// The [`RuntimeTrait`] for this `Server`.
    type Runtime: RuntimeTrait;

    /// Asynchronously return a single `Self::Transport` from a
    /// `Self::Listener`. Must be implemented.
    fn accept(&mut self) -> impl Future<Output = Result<Self::Transport>> + Send;

    /// Build an [`Info`] from the Self::Listener type. See [`Info`]
    /// for more details.
    fn init(&self, info: &mut Info) {
        let _ = info;
    }

    /// After the server has shut down, perform any housekeeping, eg
    /// unlinking a unix socket.
    fn clean_up(self) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Return this `Server`'s `Runtime`
    fn runtime() -> Self::Runtime;

    /// Build a listener from the config. The default logic for this
    /// is described elsewhere. To override the default logic, server
    /// implementations could potentially implement this directly.  To
    /// use this default logic, implement
    /// [`Server::from_tcp`] and
    /// [`Server::from_unix`].
    fn from_host_and_port(host: &str, port: u16) -> Self {
        #[cfg(unix)]
        if host.starts_with(|c| c == '/' || c == '.' || c == '~') {
            log::debug!("using unix listener at {host}");
            return UnixListener::bind(host)
                .inspect(|unix_listener| {
                    log::debug!("listening at {:?}", unix_listener.local_addr().unwrap());
                })
                .map(Self::from_unix)
                .unwrap();
        }

        let mut listen_fd = ListenFd::from_env();

        #[cfg(unix)]
        if let Ok(Some(unix_listener)) = listen_fd.take_unix_listener(0) {
            log::debug!(
                "using unix listener from systemfd environment {:?}",
                unix_listener.local_addr().unwrap()
            );
            return Self::from_unix(unix_listener);
        }

        let tcp_listener = listen_fd
            .take_tcp_listener(0)
            .ok()
            .flatten()
            .inspect(|tcp_listener| {
                log::debug!(
                    "using tcp listener from systemfd environment, listening at {:?}",
                    tcp_listener.local_addr()
                )
            })
            .unwrap_or_else(|| {
                log::debug!("using tcp listener at {host}:{port}");
                TcpListener::bind((host, port))
                    .inspect(|tcp_listener| {
                        log::debug!("listening at {:?}", tcp_listener.local_addr().unwrap())
                    })
                    .unwrap()
            });

        tcp_listener.set_nonblocking(true).unwrap();
        Self::from_tcp(tcp_listener)
    }

    /// Build a Self::Listener from a tcp listener. This is called by
    /// the [`Server::build_listener`] default implementation, and
    /// is mandatory if the default implementation is used.
    fn from_tcp(tcp_listener: TcpListener) -> Self {
        let _ = tcp_listener;
        unimplemented!()
    }

    /// Build a Self::Listener from a tcp listener. This is called by
    /// the [`Server::build_listener`] default implementation. You
    /// will want to tag an implementation of this with #[cfg(unix)].
    #[cfg(unix)]
    fn from_unix(unix_listener: UnixListener) -> Self {
        let _ = unix_listener;
        unimplemented!()
    }

    /// Implementation hook for listening for any os signals and
    /// stopping the provided [`Swansong`]. The returned future will be
    /// spawned using [`Server::spawn`]
    fn handle_signals(_swansong: Swansong) -> impl Future<Output = ()> + Send {
        async {}
    }
}
