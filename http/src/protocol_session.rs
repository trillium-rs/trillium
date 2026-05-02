use crate::{Version, h2::H2Connection, h3::H3Connection};
use std::sync::Arc;

/// The protocol-version-specific session state attached to a single request/response cycle.
///
/// HTTP/1 conns carry no session state (the underlying transport _is_ the session). HTTP/2
/// and HTTP/3 conns each multiplex many requests over a shared connection-level driver
/// (`H2Connection` / `H3Connection`) and identify their own request via a stream id.
/// Holding these as a single enum on `Conn` / `Upgrade` / `ReceivedBody` rather than four
/// parallel `Option<…>` fields enforces the "set together" invariant at the type level.
#[derive(Debug, Clone, fieldwork::Fieldwork)]
#[fieldwork(get, deref = false)]
pub enum ProtocolSession {
    /// HTTP/0.9, HTTP/1.0, or HTTP/1.1. The transport itself is the session.
    Http1,
    /// HTTP/2. The (shared) connection driver and the stream id this request rides on.
    Http2 {
        /// the [`H2Connection`] driver shared across every request on the wire
        #[field = "h2_connection"]
        connection: Arc<H2Connection>,
        /// 31-bit stream id per RFC 9113 §5.1.1
        #[field = "h2_stream_id"]
        stream_id: u32,
    },
    /// HTTP/3. The (shared) connection driver and the stream id this request rides on.
    Http3 {
        /// the [`H3Connection`] driver shared across every request on the wire
        #[field = "h3_connection"]
        connection: Arc<H3Connection>,
        /// QUIC varint stream id per RFC 9000 §2.1
        #[field = "h3_stream_id"]
        stream_id: u64,
    },
}

impl ProtocolSession {
    /// The HTTP version implied by this session. Note: synthetic conns and h1 conns both
    /// return [`Version::Http1_1`]; the more specific h1 sub-version (`Http0_9` / `Http1_0`)
    /// lives on the `Conn::version` field, which is independently tracked.
    #[must_use]
    #[allow(dead_code)]
    pub fn http_version(&self) -> Version {
        match self {
            Self::Http1 => Version::Http1_1,
            Self::Http2 { .. } => Version::Http2,
            Self::Http3 { .. } => Version::Http3,
        }
    }

    /// The h2 driver and stream id, if this session is HTTP/2. The driver is
    /// returned by clone (cheap; it's an [`Arc`]) so callers can move it into
    /// async work without juggling lifetimes against `&self`. Use
    /// [`Self::as_h2_borrowed`] to avoid the clone when borrowing suffices.
    #[must_use]
    pub fn as_h2(&self) -> Option<(Arc<H2Connection>, u32)> {
        self.as_h2_borrowed()
            .map(|(connection, stream_id)| (connection.clone(), stream_id))
    }

    /// The h2 driver (by reference) and stream id, if this session is HTTP/2.
    #[must_use]
    pub fn as_h2_borrowed(&self) -> Option<(&Arc<H2Connection>, u32)> {
        match self {
            Self::Http2 {
                connection,
                stream_id,
            } => Some((connection, *stream_id)),
            _ => None,
        }
    }

    /// The h3 driver and stream id, if this session is HTTP/3. The driver is
    /// returned by clone (cheap; it's an [`Arc`]) so callers can move it into
    /// async work without juggling lifetimes against `&self`. Use
    /// [`Self::as_h3_borrowed`] to avoid the clone when borrowing suffices.
    #[must_use]
    pub fn as_h3(&self) -> Option<(Arc<H3Connection>, u64)> {
        self.as_h3_borrowed()
            .map(|(connection, stream_id)| (connection.clone(), stream_id))
    }

    /// The h3 driver (by reference) and stream id, if this session is HTTP/3.
    #[must_use]
    pub fn as_h3_borrowed(&self) -> Option<(&Arc<H3Connection>, u64)> {
        match self {
            Self::Http3 {
                connection,
                stream_id,
            } => Some((connection, *stream_id)),
            _ => None,
        }
    }
}
