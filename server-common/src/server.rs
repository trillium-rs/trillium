use crate::{RuntimeTrait, Swansong, Transport, UdpTransport};
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

    /// The async UDP socket type for this server. Used by QUIC adapters
    /// for HTTP/3 support. Runtimes that do not support UDP should set
    /// this to `()`.
    type UdpTransport: UdpTransport;

    /// Asynchronously return a single `Self::Transport` from a
    /// `Self::Listener`. Must be implemented.
    fn accept(&mut self) -> impl Future<Output = Result<Self::Transport>> + Send;

    /// Populate [`Info`] with data from this server, such as the bound socket address.
    /// Called during server startup before any connections are accepted.
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
        match resolve_listener(host, port).unwrap() {
            PreboundListener::Tcp(tcp_listener) => Self::from_tcp(tcp_listener),
            #[cfg(unix)]
            PreboundListener::Unix(unix_listener) => Self::from_unix(unix_listener),
        }
    }

    /// Build a Self::Listener from a tcp listener. This is called by
    /// the [`Server::from_host_and_port`] default implementation, and
    /// is mandatory if the default implementation is used.
    fn from_tcp(tcp_listener: TcpListener) -> Self {
        let _ = tcp_listener;
        unimplemented!()
    }

    /// Build a `Self` from a unix listener. This is called by
    /// the [`Server::from_host_and_port`] default implementation. You
    /// will want to tag an implementation of this with `#[cfg(unix)]`.
    #[cfg(unix)]
    fn from_unix(unix_listener: UnixListener) -> Self {
        let _ = unix_listener;
        unimplemented!()
    }

    /// Implementation hook for listening for any os signals and
    /// stopping the provided [`Swansong`]. The returned future will be
    /// spawned using [`RuntimeTrait::spawn`]
    fn handle_signals(_swansong: Swansong) -> impl Future<Output = ()> + Send {
        async {}
    }
}

/// A listener claimed by [`resolve_listener`] before it has been adopted into a runtime: either a
/// freshly-bound (or inherited) TCP listener, or, on unix, a Unix-domain-socket listener.
#[derive(Debug)]
pub(crate) enum PreboundListener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixListener),
}

/// Resolve a `host`/`port` into a freshly-claimed std listener following trillium's 12-factor
/// binding rules:
///
/// - on unix, a `host` beginning with `/`, `.`, or `~` is treated as a path and bound as a
///   Unix-domain socket (the `port` is ignored);
/// - otherwise, a listener inherited via the `LISTEN_FDS` socket-activation protocol at fd index 0
///   (unix listener preferred, then TCP) is adopted if present;
/// - otherwise a TCP listener is bound to `host:port`.
///
/// This is the fallible core of the default [`Server::from_host_and_port`] (which `.unwrap()`s the
/// result, preserving its panic-on-failure contract) and of the multi-listener builder's `bind_env`
/// (which propagates the error). TCP listeners are set non-blocking; the Unix path mirrors the
/// historical `from_host_and_port` behavior and does not.
pub(crate) fn resolve_listener(host: &str, port: u16) -> Result<PreboundListener> {
    #[cfg(unix)]
    if host.starts_with(['/', '.', '~']) {
        log::debug!("using unix listener at {host}");
        let unix_listener = UnixListener::bind(host)?;
        log::debug!("listening at {:?}", unix_listener.local_addr());
        return Ok(PreboundListener::Unix(unix_listener));
    }

    let mut listen_fd = ListenFd::from_env();

    #[cfg(unix)]
    if let Ok(Some(unix_listener)) = listen_fd.take_unix_listener(0) {
        log::debug!(
            "using unix listener from systemfd environment {:?}",
            unix_listener.local_addr()
        );
        return Ok(PreboundListener::Unix(unix_listener));
    }

    let tcp_listener = match listen_fd.take_tcp_listener(0).ok().flatten() {
        Some(tcp_listener) => {
            log::debug!(
                "using tcp listener from systemfd environment, listening at {:?}",
                tcp_listener.local_addr()
            );
            tcp_listener
        }
        None => {
            log::debug!("using tcp listener at {host}:{port}");
            let tcp_listener = TcpListener::bind((host, port))?;
            log::debug!("listening at {:?}", tcp_listener.local_addr());
            tcp_listener
        }
    };

    tcp_listener.set_nonblocking(true)?;
    Ok(PreboundListener::Tcp(tcp_listener))
}
