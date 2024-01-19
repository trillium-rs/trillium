//! Support for client-side WebSockets

use std::fmt::{self, Display};
use std::ops::{Deref, DerefMut};

use trillium_http::{
    KnownHeaderName::{self, SecWebsocketAccept, SecWebsocketKey},
    Status, Upgrade,
};
use trillium_websockets::{websocket_accept_hash, websocket_key, Role};

use crate::{Conn, WebSocketConfig, WebSocketConn};

pub use trillium_websockets::Message;

impl Conn {
    fn set_websocket_upgrade_headers(&mut self) {
        let h = self.request_headers_mut();
        h.try_insert(KnownHeaderName::Upgrade, "websocket");
        h.try_insert(KnownHeaderName::Connection, "upgrade");
        h.try_insert(KnownHeaderName::SecWebsocketVersion, "13");
        h.try_insert(SecWebsocketKey, websocket_key());
    }

    /// Turn this `Conn` into a [`WebSocketConn`]
    ///
    /// If the request has not yet been sent, this will call `with_websocket_upgrade_headers()` and
    /// then send the request.
    pub async fn into_websocket(self) -> Result<WebSocketConn, WebSocketUpgradeError> {
        self.into_websocket_with_config(WebSocketConfig::default())
            .await
    }

    /// Turn this `Conn` into a [`WebSocketConn`], with a custom [`WebSocketConfig`]
    ///
    /// If the request has not yet been sent, this will call `with_websocket_upgrade_headers()` and
    /// then send the request.
    pub async fn into_websocket_with_config(
        mut self,
        config: WebSocketConfig,
    ) -> Result<WebSocketConn, WebSocketUpgradeError> {
        let status = match self.status() {
            Some(status) => status,
            None => {
                self.set_websocket_upgrade_headers();
                if let Err(e) = (&mut self).await {
                    return Err(WebSocketUpgradeError::new(self, e.into()));
                }
                self.status().expect("Response did not include status")
            }
        };
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
}

/// The kind of error that occurred when attempting a websocket upgrade
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
/// An Error type that represents all exceptional conditions that can be encoutered in the operation
/// of this crate
pub enum ErrorKind {
    /// an HTTP error attempting to make the request
    #[error(transparent)]
    Http(#[from] trillium_http::Error),

    /// Response didn't have status 101 (Switching Protocols)
    #[error("Expected status 101 (Switching Protocols), got {0}")]
    Status(Status),

    /// Response Sec-WebSocket-Accept was missing or invalid; generally a server bug
    #[error("Response Sec-WebSocket-Accept was missing or invalid")]
    InvalidAccept,
}

/// An attempted upgrade to a WebSocket failed. You can transform this back into the Conn with
/// [`From::from`]/[`Into::into`], if you need to look at the server response.
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

impl std::error::Error for WebSocketUpgradeError {}

impl Display for WebSocketUpgradeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}
