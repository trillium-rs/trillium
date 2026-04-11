//! Support for client-side WebTransport (RFC 9220 + draft-ietf-webtrans-http3).
//!
//! Multiple sessions to the same origin coalesce onto a single underlying QUIC connection;
//! each `into_webtransport` opens a new bidi stream for the extended CONNECT and registers
//! the new session with the connection's per-origin
//! [`Router`][trillium_webtransport::Router].

use crate::{Client, Conn, IntoUrl};
use std::{
    borrow::Cow,
    error::Error,
    fmt::{self, Display},
    ops::{Deref, DerefMut},
};
use trillium_http::{Method, Status, Version};
use trillium_server_common::h3::web_transport::WebTransportDispatcher;
use trillium_webtransport::{DEFAULT_MAX_DATAGRAM_BUFFER, Router, WebTransportConnection};

impl Client {
    /// Build a [`Conn`] preconfigured for an extended-CONNECT WebTransport handshake to `url`.
    ///
    /// Sets the method to CONNECT, the `:protocol` pseudo-header to `webtransport`, and pins
    /// the http version to HTTP/3. The conn has not yet been sent — chain
    /// [`Conn::with_request_header`](crate::Conn::with_request_header) etc. as usual, then
    /// `await` it via [`Conn::into_webtransport`] to complete the upgrade and obtain a
    /// [`WebTransportConnection`].
    pub fn webtransport(&self, url: impl IntoUrl) -> Conn {
        let mut conn = self.build_conn(Method::Connect, url);
        conn.http_version = Version::Http3;
        conn.protocol = Some(Cow::Borrowed("webtransport"));
        conn
    }
}

impl Conn {
    /// Execute this conn as a WebTransport extended CONNECT and return the resulting session.
    ///
    /// On success, the conn's QUIC connection is reused for any subsequent
    /// `webtransport(...)` calls to the same origin: each new session opens an additional
    /// bidi stream on the existing QUIC connection rather than dialing fresh, matching how
    /// HTTP/3 request multiplexing already works.
    ///
    /// This is an *execution* method. It must be called on a conn that has not yet been
    /// awaited; calling it after the conn has executed returns
    /// [`ErrorKind::AlreadyExecuted`].
    pub async fn into_webtransport(
        mut self,
    ) -> Result<WebTransportConnection, WebTransportConnectError> {
        if self.status().is_some() {
            return Err(WebTransportConnectError::new(
                self,
                ErrorKind::AlreadyExecuted,
            ));
        }

        if self.method() != Method::Connect || self.protocol.as_deref() != Some("webtransport") {
            return Err(WebTransportConnectError::new(self, ErrorKind::InvalidConn));
        }

        // The peer-capability check (RFC 9220 §3 — server must have advertised
        // SETTINGS_ENABLE_CONNECT_PROTOCOL, plus SETTINGS_ENABLE_WEBTRANSPORT and
        // SETTINGS_H3_DATAGRAM for WT) lives inside `try_exec_h3`, where it can park on
        // the peer's first SETTINGS *before* opening the CONNECT stream. The dispatcher is
        // also lazy-initialized there, so any inbound WT streams that arrive during the
        // round-trip land in the dispatcher's `Buffering` state.
        if let Err(e) = (&mut self).await {
            let kind = match e {
                trillium_http::Error::ExtendedConnectUnsupported => {
                    ErrorKind::ExtendedConnectUnsupported
                }
                other => other.into(),
            };
            return Err(WebTransportConnectError::new(self, kind));
        }

        let status = self.status().expect("response did not include status");
        if status != Status::Ok {
            return Err(WebTransportConnectError::new(
                self,
                ErrorKind::Status(status),
            ));
        }

        let Some(entry) = self.wt_pool_entry.take() else {
            // Should not happen: try_exec_h3 populates this for any conn whose protocol is
            // webtransport.
            return Err(WebTransportConnectError::new(self, ErrorKind::InvalidConn));
        };
        let Some((h3_connection, session_id)) = self.protocol_session.as_h3() else {
            return Err(WebTransportConnectError::new(self, ErrorKind::InvalidConn));
        };
        let dispatcher = entry
            .dispatcher
            .get_or_init(WebTransportDispatcher::new)
            .clone();

        // Get-or-init the router and start the routing task. Idempotent across sessions on
        // the same QUIC connection.
        let runtime = self.config.runtime();
        let max_datagram_buffer = DEFAULT_MAX_DATAGRAM_BUFFER;
        let Some(router) = dispatcher.get_or_init_with(|| Router::new(max_datagram_buffer)) else {
            return Err(WebTransportConnectError::new(
                self,
                ErrorKind::DispatcherTypeMismatch,
            ));
        };
        router
            .clone()
            .spawn_routing_task(entry.quic_conn.clone(), runtime.clone());

        // Register the session and pull receivers.
        let (bidi_rx, uni_rx, datagram_rx) = router.sessions().lock().await.register(session_id);

        let session_swansong = h3_connection.swansong().child();
        let path = self.path.clone();
        let authority = self.authority.clone();

        // Drop the inner Conn, retaining the parts we need for the WebTransportConnection.
        let request_headers = std::mem::take(&mut self.request_headers);
        let response_headers = std::mem::take(&mut self.response_headers);
        let state = std::mem::take(&mut self.state);

        Ok(WebTransportConnection::new(
            session_id,
            bidi_rx,
            uni_rx,
            datagram_rx,
            session_swansong,
            request_headers,
            response_headers,
            state,
            path,
            authority,
            h3_connection,
            entry.quic_conn,
            runtime,
        ))
    }
}

/// The kind of error that occurred when attempting a WebTransport upgrade.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// An HTTP error occurred attempting to make the request.
    #[error(transparent)]
    Http(#[from] trillium_http::Error),

    /// Response did not have status 200 (RFC 9220 — extended-CONNECT success).
    #[error("Unexpected response status {0} for WebTransport upgrade")]
    Status(Status),

    /// `into_webtransport` was called on a conn that had already been executed (its status
    /// was already set), or that was not constructed via [`Client::webtransport`] / lacks the
    /// required `:protocol` and method state.
    #[error(
        "Conn is not in a valid state for WebTransport upgrade — build via `Client::webtransport` \
         and do not await separately"
    )]
    AlreadyExecuted,

    /// The conn was not constructed via [`Client::webtransport`] or has been mutated into a
    /// state that no longer reflects an extended-CONNECT WebTransport request.
    #[error("Conn is not configured for a WebTransport upgrade")]
    InvalidConn,

    /// The peer did not advertise the SETTINGS required for WebTransport over HTTP/3
    /// (`SETTINGS_ENABLE_CONNECT_PROTOCOL`, `SETTINGS_ENABLE_WEBTRANSPORT`, and
    /// `SETTINGS_H3_DATAGRAM`).
    #[error("peer does not support WebTransport over HTTP/3")]
    ExtendedConnectUnsupported,

    /// Internal: the QUIC connection's [`WebTransportDispatcher`] was already initialized
    /// with a router of an unexpected type. Indicates a bug — should not happen in practice.
    #[error("dispatcher already initialized with an incompatible handler type")]
    DispatcherTypeMismatch,
}

/// An attempted WebTransport upgrade failed.
///
/// You can recover the underlying [`Conn`] via [`From::from`]/[`Into::into`] to inspect
/// the server's response.
#[derive(Debug)]
pub struct WebTransportConnectError {
    /// The kind of error that occurred.
    pub kind: ErrorKind,
    conn: Box<Conn>,
}

impl WebTransportConnectError {
    fn new(conn: Conn, kind: ErrorKind) -> Self {
        Self {
            conn: Box::new(conn),
            kind,
        }
    }
}

impl From<WebTransportConnectError> for Conn {
    fn from(value: WebTransportConnectError) -> Self {
        *value.conn
    }
}

impl Deref for WebTransportConnectError {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl DerefMut for WebTransportConnectError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

impl Error for WebTransportConnectError {}

impl Display for WebTransportConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}
