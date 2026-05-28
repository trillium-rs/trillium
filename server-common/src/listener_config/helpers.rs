use crate::server::{PreboundListener, resolve_listener};
use std::{collections::HashMap, io, net::TcpListener as StdTcpListener};

/// Claim the inherited TCP listener at the given file-descriptor index, via the `LISTEN_FDS`
/// socket-activation protocol. Errors if no inherited TCP listener is present at that index.
pub(super) fn take_inherited_fd(index: usize) -> io::Result<StdTcpListener> {
    listenfd::ListenFd::from_env()
        .take_tcp_listener(index)?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no inherited TCP listener at fd index {index}"),
            )
        })
}

/// Resolve a listener from the environment per trillium's 12-factor conventions, shared by
/// [`ListenerConfig::bind_env`](super::ListenerConfig::bind_env) and
/// [`ListenerConfig::bind_env_tls`](super::ListenerConfig::bind_env_tls): `HOST` (default
/// `localhost`), `PORT` (default `8080`, parse failure surfaced as an error), then the uds /
/// `LISTEN_FD` / tcp resolution in [`resolve_listener`].
pub(super) fn resolve_env_listener() -> io::Result<PreboundListener> {
    let host = std::env::var("HOST").unwrap_or_else(|_| "localhost".into());
    let port = match std::env::var("PORT") {
        Ok(port) => port.parse().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("PORT must be an unsigned integer: {e}"),
            )
        })?,
        Err(_) => 8080,
    };
    resolve_listener(&host, port)
}

/// Resolve `bind_quic` / `with_alt_svc` declarations into `from_port → &'static str` alt-svc
/// header values. A TCP/TLS listener and a quic listener bound to the same port auto-pair; any
/// non-matching pairing must be added with `with_alt_svc`. Values sharing a `from` port are merged
/// into one comma-joined header value, leaked once so it can be shared cheaply across responses
/// and is eligible for h2/h3 dynamic-table reuse.
///
/// Dangling references in `with_alt_svc` pairs (from-port not bound TCP, or to-port not bound
/// QUIC) are warned but otherwise included; the user knows their topology better than we do.
pub(super) fn build_alt_svc_map(
    listener_ports: &[u16],
    quic_ports: &[u16],
    explicit_pairs: &[(u16, u16)],
) -> HashMap<u16, &'static str> {
    let mut pairs_by_from: HashMap<u16, Vec<u16>> = HashMap::new();

    for &p in quic_ports {
        if listener_ports.contains(&p) {
            pairs_by_from.entry(p).or_default().push(p);
        }
    }

    for &(from, to) in explicit_pairs {
        if !listener_ports.contains(&from) {
            log::warn!("with_alt_svc({from}, {to}): no TCP listener bound on port {from}");
        }
        if !quic_ports.contains(&to) {
            log::warn!("with_alt_svc({from}, {to}): no QUIC listener bound on port {to}");
        }
        let tos = pairs_by_from.entry(from).or_default();
        if !tos.contains(&to) {
            tos.push(to);
        }
    }

    pairs_by_from
        .into_iter()
        .map(|(from, tos)| {
            let value = tos
                .iter()
                .map(|t| format!("h3=\":{t}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let leaked: &'static str = Box::leak(value.into_boxed_str());
            (from, leaked)
        })
        .collect()
}
