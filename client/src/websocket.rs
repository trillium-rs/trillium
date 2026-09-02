//! Support for client-side WebSockets

use crate::{Conn, WebSocketConfig, WebSocketConn};
use std::{
    borrow::Cow,
    error::Error,
    fmt::{self, Display},
    ops::{Deref, DerefMut},
};
use trillium_http::{
    KnownHeaderName::{SecWebsocketAccept, SecWebsocketKey, SecWebsocketVersion},
    Status, Upgrade, Version,
};
pub use trillium_websockets::Message;
use trillium_websockets::{Role, websocket_accept_hash};

impl Conn {
    /// Attempt to transform this `Conn` into a [`WebSocketConn`].
    ///
    /// This is an *execution* method: calling it on a conn that has already been awaited
    /// returns [`ErrorKind::AlreadyExecuted`]. Build the conn, then call this — don't await
    /// it yourself first.
    ///
    /// The handshake is an `Upgrade` over HTTP/1.1 and an extended CONNECT over HTTP/2 and
    /// HTTP/3. Which one is sent follows the same protocol selection as any other request,
    /// with one addition: a peer that speaks h2 or h3 but does not support extended CONNECT
    /// is retried as an HTTP/1.1 upgrade, unless
    /// [`strict_http_version`](Conn::strict_http_version) is on, in which case it yields
    /// [`ErrorKind::ExtendedConnectUnsupported`]. See the crate-level [Protocol
    /// selection][crate#protocol-selection] documentation.
    pub async fn into_websocket(self) -> Result<WebSocketConn, WebSocketUpgradeError> {
        self.into_websocket_with_config(WebSocketConfig::default())
            .await
    }

    /// Like [`Conn::into_websocket`] but with a caller-supplied [`WebSocketConfig`].
    pub async fn into_websocket_with_config(
        mut self,
        config: WebSocketConfig,
    ) -> Result<WebSocketConn, WebSocketUpgradeError> {
        if self.status().is_some() {
            return Err(WebSocketUpgradeError::new(self, ErrorKind::AlreadyExecuted));
        }

        // Only the protocol-neutral parts of the handshake are set here. The h1-only
        // `Upgrade`/`Connection`/`Sec-WebSocket-Key` headers are added when the request is
        // rendered for h1, so a request that goes out as an extended CONNECT never carries them.
        self.protocol = Some(Cow::Borrowed("websocket"));
        self.request_headers_mut()
            .try_insert(SecWebsocketVersion, "13");

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
        match self.http_version() {
            Version::Http2 | Version::Http3 => {
                if status != Status::Ok {
                    return Err(WebSocketUpgradeError::new(self, ErrorKind::Status(status)));
                }
            }
            _ => {
                if status != Status::SwitchingProtocols {
                    return Err(WebSocketUpgradeError::new(self, ErrorKind::Status(status)));
                }
                let key = self
                    .request_headers()
                    .get_str(SecWebsocketKey)
                    .expect("h1 websocket request did not include Sec-WebSocket-Key");
                let accept_key = websocket_accept_hash(key);
                if self.response_headers().get_str(SecWebsocketAccept) != Some(&accept_key) {
                    return Err(WebSocketUpgradeError::new(self, ErrorKind::InvalidAccept));
                }
            }
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
    /// connection. Only surfaced with [`strict_http_version`](Conn::strict_http_version) on;
    /// otherwise the client retries as an HTTP/1.1 upgrade instead.
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
