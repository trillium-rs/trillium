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
    /// Set the appropriate headers for upgrading to a WebSocket
    pub fn with_websocket_upgrade_headers(self) -> Conn {
        self.with_header(KnownHeaderName::Upgrade, "websocket")
            .with_header(KnownHeaderName::Connection, "upgrade")
            .with_header(KnownHeaderName::SecWebsocketVersion, "13")
            .with_header(SecWebsocketKey, websocket_key())
    }

    /// Turn this `Conn` into a [`WebSocketConn`]
    pub async fn into_websocket(self) -> Result<WebSocketConn, WebSocketUpgradeError> {
        self.into_websocket_with_config(WebSocketConfig::default())
            .await
    }

    /// Turn this `Conn` into a [`WebSocketConn`], with a custom [`WebSocketConfig`]
    pub async fn into_websocket_with_config(
        self,
        config: WebSocketConfig,
    ) -> Result<WebSocketConn, WebSocketUpgradeError> {
        let status = self
            .status()
            .expect("into_websocket() with request not yet sent; remember to call .await");
        if status != Status::SwitchingProtocols {
            return Err(WebSocketUpgradeError::new(
                self,
                "Expected status 101 (Switching Protocols)",
            ));
        }
        let Some(key) = self.request_headers().get_str(SecWebsocketKey) else {
            return Err(WebSocketUpgradeError::new(
                self,
                "Request did not include Sec-WebSocket-Key",
            ));
        };
        let accept_key = websocket_accept_hash(key);
        if self.response_headers().get_str(SecWebsocketAccept) != Some(&accept_key) {
            return Err(WebSocketUpgradeError::new(
                self,
                "Response did not contain valid Sec-WebSocket-Accept",
            ));
        }
        let peer_ip = self.peer_addr().map(|addr| addr.ip());
        let mut conn = WebSocketConn::new(Upgrade::from(self), Some(config), Role::Client).await;
        conn.set_peer_ip(peer_ip);
        Ok(conn)
    }
}

/// An attempted upgrade to a WebSocket failed. You can transform this back into the Conn with
/// [`From::from`]/[`Into::into`].
#[derive(Debug)]
pub struct WebSocketUpgradeError(Box<Conn>, &'static str);

impl WebSocketUpgradeError {
    fn new(conn: Conn, msg: &'static str) -> Self {
        Self(Box::new(conn), msg)
    }
}

impl From<WebSocketUpgradeError> for Conn {
    fn from(value: WebSocketUpgradeError) -> Self {
        *value.0
    }
}

impl Deref for WebSocketUpgradeError {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for WebSocketUpgradeError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl std::error::Error for WebSocketUpgradeError {}

impl Display for WebSocketUpgradeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.1)
    }
}
