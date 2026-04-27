//! Whether the driver is running as the server side or the client side of an HTTP/2
//! connection. The handful of role-asymmetric branch points in the driver — preface
//! direction, HEADERS-on-unknown-id semantics, HEADERS-on-known-id semantics — route
//! through a single match on this enum each.

/// Whether this driver is servicing a peer that dialled us (server role) or a peer we
/// dialled (client role). Routes the handful of role-asymmetric driver concerns — preface
/// direction, HEADERS-on-unknown-id semantics, HEADERS-on-known-id semantics — through a
/// single match point each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    /// Driver was handed a transport from an accepting listener — we read the client
    /// preface, treat peer-initiated (odd-id) streams as new requests, and treat HEADERS
    /// on a known stream as trailers.
    Server,
    /// Driver was handed a transport from an outbound dial — we write the client preface,
    /// open streams with locally-allocated odd ids, and treat HEADERS on one of our
    /// streams as the response headers (first arrival) or trailers (second). Produced by
    /// [`H2Connection::run_client`][super::H2Connection::run_client], which is gated
    /// behind the `unstable` feature — without that feature the variant is defined but
    /// never constructed.
    #[cfg_attr(not(feature = "unstable"), allow(dead_code))]
    Client,
}
