//! Support for client-side WebSockets

use crate::{Conn, WebSocketConfig, WebSocketConn};
use std::{
    borrow::Cow,
    error::Error,
    fmt::{self, Display},
    ops::{Deref, DerefMut},
};
use trillium_http::{
    KnownHeaderName::{
        Connection, SecWebsocketAccept, SecWebsocketKey, SecWebsocketVersion,
        Upgrade as UpgradeHeader,
    },
    Method, Status, Upgrade, Version,
};
pub use trillium_websockets::Message;
use trillium_websockets::{Role, websocket_accept_hash, websocket_key};

impl Conn {
    fn set_websocket_upgrade_headers_h1(&mut self) {
        let headers = self.request_headers_mut();
        headers.try_insert(UpgradeHeader, "websocket");
        headers.try_insert(Connection, "upgrade");
        headers.try_insert(SecWebsocketVersion, "13");
        headers.try_insert(SecWebsocketKey, websocket_key());
    }

    /// Attempt to transform this `Conn` into a [`WebSocketConn`].
    ///
    /// This is an *execution* method: calling it on a conn that has already been awaited
    /// returns [`ErrorKind::AlreadyExecuted`]. Build the conn, then call this — don't await
    /// it yourself first.
    ///
    /// Protocol selection follows the conn's [`http_version`][Conn::http_version] hint:
    /// `Http2` and `Http3` use the extended-CONNECT bootstrap (RFC 8441 over h2, RFC 9220
    /// over h3); the default uses an h1 `Upgrade` handshake (RFC 6455). If the peer is h2/h3
    /// but doesn't advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL`, the upgrade hard-errors —
    /// there is no silent fallback to h1 from a non-capable peer.
    pub async fn into_websocket(self) -> Result<WebSocketConn, WebSocketUpgradeError> {
        self.into_websocket_with_config(WebSocketConfig::default())
            .await
    }

    /// Like [`Conn::into_websocket`] but with a caller-supplied [`WebSocketConfig`].
    pub async fn into_websocket_with_config(
        self,
        config: WebSocketConfig,
    ) -> Result<WebSocketConn, WebSocketUpgradeError> {
        if self.status().is_some() {
            return Err(WebSocketUpgradeError::new(self, ErrorKind::AlreadyExecuted));
        }

        match self.http_version() {
            Version::Http2 | Version::Http3 => self.into_websocket_extended_connect(config).await,
            _ => self.into_websocket_h1(config).await,
        }
    }

    async fn into_websocket_h1(
        mut self,
        config: WebSocketConfig,
    ) -> Result<WebSocketConn, WebSocketUpgradeError> {
        self.set_websocket_upgrade_headers_h1();
        if let Err(e) = (&mut self).await {
            return Err(WebSocketUpgradeError::new(self, e.into()));
        }
        let status = self.status().expect("Response did not include status");
        if status != Status::SwitchingProtocols {
            return Err(WebSocketUpgradeError::new(self, ErrorKind::Status(status)));
        }
        let key = self
            .request_headers()
            .get_str(SecWebsocketKey)
            .expect("Request did not include Sec-WebSocket-Key");
        let accept_key = websocket_accept_hash(key);
        if self.response_headers().get_str(SecWebsocketAccept) != Some(&accept_key) {
            return Err(WebSocketUpgradeError::new(self, ErrorKind::InvalidAccept));
        }
        let peer_ip = self.peer_addr().map(|addr| addr.ip());
        let mut conn = WebSocketConn::new(Upgrade::from(self), Some(config), Role::Client).await;
        conn.set_peer_ip(peer_ip);
        Ok(conn)
    }

    async fn into_websocket_extended_connect(
        mut self,
        config: WebSocketConfig,
    ) -> Result<WebSocketConn, WebSocketUpgradeError> {
        // Extended CONNECT carries `Sec-WebSocket-Version: 13` and the optional
        // `Sec-WebSocket-Protocol`, but skips the `Sec-WebSocket-Key` / `Sec-WebSocket-Accept`
        // SHA1 dance — those are h1-only artifacts. The `Connection: upgrade` /
        // `Upgrade: websocket` headers are likewise h1-only and would be stripped by
        // `finalize_headers_h2` / `_h3` even if we set them.
        self.request_headers_mut()
            .try_insert(SecWebsocketVersion, "13");
        self.set_method(Method::Connect);
        self.protocol = Some(Cow::Borrowed("websocket"));

        // The peer-capability gate (server must have advertised
        // `SETTINGS_ENABLE_CONNECT_PROTOCOL` before the client may send a `:protocol`
        // HEADERS) lives inside the h2 client send path, where it can park on the peer's
        // first SETTINGS *before* putting any HEADERS on the wire. A "not supported"
        // outcome surfaces here as `Error::ExtendedConnectUnsupported`.
        if let Err(e) = (&mut self).await {
            let kind = match e {
                trillium_http::Error::ExtendedConnectUnsupported => {
                    ErrorKind::ExtendedConnectUnsupported
                }
                other => other.into(),
            };
            return Err(WebSocketUpgradeError::new(self, kind));
        }

        let status = self.status().expect("Response did not include status");
        if status != Status::Ok {
            return Err(WebSocketUpgradeError::new(self, ErrorKind::Status(status)));
        }

        let peer_ip = self.peer_addr().map(|addr| addr.ip());
        let mut conn = WebSocketConn::new(Upgrade::from(self), Some(config), Role::Client).await;
        conn.set_peer_ip(peer_ip);
        Ok(conn)
    }
}

/// The kind of error that occurred when attempting a websocket upgrade
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum ErrorKind {
    /// an HTTP error attempting to make the request
    #[error(transparent)]
    Http(#[from] trillium_http::Error),

    /// Response didn't have the expected status (101 Switching Protocols for h1, 200 OK for
    /// h2/h3 extended CONNECT).
    #[error("Unexpected response status {0} for websocket upgrade")]
    Status(Status),

    /// Response Sec-WebSocket-Accept was missing or invalid; generally a server bug
    #[error("Response Sec-WebSocket-Accept was missing or invalid")]
    InvalidAccept,

    /// `into_websocket` was called on a `Conn` that had already been executed (its status is
    /// already set). The websocket upgrade *is* the execution; build the conn and call
    /// `into_websocket` directly without awaiting first.
    #[error(
        "Conn::into_websocket called after execution — build the conn and await into_websocket \
         instead of awaiting the conn separately"
    )]
    AlreadyExecuted,

    /// The h2 or h3 peer did not advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL = 1`, so the
    /// extended-CONNECT bootstrap (RFC 8441 over h2, RFC 9220 over h3) is not available on this
    /// connection.
    #[error("peer does not support extended CONNECT")]
    ExtendedConnectUnsupported,
}

/// An attempted upgrade to a WebSocket failed.
///
/// You can transform this back into the Conn with [`From::from`]/[`Into::into`], if you need to
/// look at the server response.
#[derive(Debug)]
pub struct WebSocketUpgradeError {
    /// The kind of error that occurred
    pub kind: ErrorKind,
    conn: Box<Conn>,
}

impl WebSocketUpgradeError {
    fn new(conn: Conn, kind: ErrorKind) -> Self {
        let conn = Box::new(conn);
        Self { conn, kind }
    }
}

impl From<WebSocketUpgradeError> for Conn {
    fn from(value: WebSocketUpgradeError) -> Self {
        *value.conn
    }
}

impl Deref for WebSocketUpgradeError {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}
impl DerefMut for WebSocketUpgradeError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

impl Error for WebSocketUpgradeError {}

impl Display for WebSocketUpgradeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}
