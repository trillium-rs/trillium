#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
# A websocket trillium handler

There are three primary ways to use this crate

## With an async function that receives a [`WebSocketConn`](crate::WebSocketConn)

This is the simplest way to use trillium websockets, but does not
provide any of the affordances that implementing the
[`WebSocketHandler`] trait does. It is best for very simple websockets
or for usages that require moving the WebSocketConn elsewhere in an
application. The WebSocketConn is fully owned at this point, and will
disconnect when dropped, not when the async function passed to
`websocket` completes.

```
use futures_lite::stream::StreamExt;
use trillium_websockets::{Message, WebSocketConn, websocket};

let handler = websocket(|mut conn: WebSocketConn| async move {
    while let Some(Ok(Message::Text(input))) = conn.next().await {
        conn.send_string(format!("received your message: {}", &input)).await;
    }
});
# // tests at tests/tests.rs for example simplicity
```


## Implementing [`WebSocketHandler`](crate::WebSocketHandler)

[`WebSocketHandler`] provides support for sending outbound messages as a
stream, and simplifies common patterns like executing async code on
received messages.

## Using [`JsonWebSocketHandler`](crate::JsonWebSocketHandler)

[`JsonWebSocketHandler`] provides a thin serialization and
deserialization layer on top of [`WebSocketHandler`] for this common
use case.  See the [`JsonWebSocketHandler`] documentation for example
usage. In order to use this trait, the `json` cargo feature must be
enabled.

*/

mod bidirectional_stream;
mod websocket_connection;
mod websocket_handler;

pub use async_tungstenite::{
    self,
    tungstenite::{
        self,
        protocol::{Role, WebSocketConfig},
        Message,
    },
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use bidirectional_stream::{BidirectionalStream, Direction};
use futures_lite::stream::StreamExt;
use sha1::{Digest, Sha1};
use std::{
    net::IpAddr,
    ops::{Deref, DerefMut},
};
use trillium::{
    Conn, Handler,
    KnownHeaderName::{
        Connection, SecWebsocketAccept, SecWebsocketKey, SecWebsocketProtocol, SecWebsocketVersion,
        Upgrade as UpgradeHeader,
    },
    Status, Upgrade,
};
pub use websocket_connection::WebSocketConn;
pub use websocket_handler::WebSocketHandler;

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
/// An Error type that represents all exceptional conditions that can be encoutered in the operation
/// of this crate
pub enum Error {
    #[error(transparent)]
    /// an error in the underlying websocket implementation
    WebSocket(#[from] tungstenite::Error),

    #[cfg(feature = "json")]
    #[error(transparent)]
    /// an error in json serialization or deserialization
    Json(#[from] serde_json::Error),
}

/// a Result type for this crate
pub type Result<T = Message> = std::result::Result<T, Error>;

#[cfg(feature = "json")]
mod json;

#[cfg(feature = "json")]
pub use json::{json_websocket, JsonHandler, JsonWebSocketHandler};

/**
The trillium handler.
See crate-level docs for example usage.
*/
#[derive(Debug)]
pub struct WebSocket<H> {
    handler: H,
    protocols: Vec<String>,
    config: Option<WebSocketConfig>,
    required: bool,
}

impl<H> Deref for WebSocket<H> {
    type Target = H;

    fn deref(&self) -> &Self::Target {
        &self.handler
    }
}

impl<H> DerefMut for WebSocket<H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.handler
    }
}

/**
Builds a new trillium handler from the provided
WebSocketHandler. Alias for [`WebSocket::new`]
*/
pub fn websocket<H>(websocket_handler: H) -> WebSocket<H>
where
    H: WebSocketHandler,
{
    WebSocket::new(websocket_handler)
}

impl<H> WebSocket<H>
where
    H: WebSocketHandler,
{
    /// Build a new WebSocket with an async handler function that
    /// receives a [`WebSocketConn`]
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            protocols: Default::default(),
            config: None,
            required: false,
        }
    }

    /// `protocols` is a sequence of known protocols. On successful handshake,
    /// the returned response headers contain the first protocol in this list
    /// which the server also knows.
    pub fn with_protocols(self, protocols: &[&str]) -> Self {
        Self {
            protocols: protocols.iter().map(ToString::to_string).collect(),
            ..self
        }
    }

    /// configure the websocket protocol
    pub fn with_protocol_config(self, config: WebSocketConfig) -> Self {
        Self {
            config: Some(config),
            ..self
        }
    }

    /// configure this handler to halt and send back a [`426 Upgrade
    /// Required`][Status::UpgradeRequired] if a websocket cannot be negotiated
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

struct IsWebsocket;

#[cfg(test)]
mod tests;

// this is a workaround for the fact that Upgrade is a public struct,
// so adding peer_ip to that struct would be a breaking change. We
// stash a copy in state for now.
struct WebsocketPeerIp(Option<IpAddr>);

impl<H> Handler for WebSocket<H>
where
    H: WebSocketHandler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        if !upgrade_requested(&conn) {
            if self.required {
                return conn.with_status(Status::UpgradeRequired).halt();
            } else {
                return conn;
            }
        }

        let websocket_peer_ip = WebsocketPeerIp(conn.peer_ip());

        let Some(sec_websocket_key) = conn.request_headers().get_str(SecWebsocketKey) else {
            return conn.with_status(Status::BadRequest).halt();
        };
        let sec_websocket_accept = websocket_accept_hash(sec_websocket_key);

        let protocol = websocket_protocol(&conn, &self.protocols);

        let headers = conn.response_headers_mut();

        headers.extend([
            (UpgradeHeader, "websocket"),
            (Connection, "Upgrade"),
            (SecWebsocketVersion, "13"),
        ]);

        headers.insert(SecWebsocketAccept, sec_websocket_accept);

        if let Some(protocol) = protocol {
            headers.insert(SecWebsocketProtocol, protocol);
        }

        conn.halt()
            .with_state(websocket_peer_ip)
            .with_state(IsWebsocket)
            .with_status(Status::SwitchingProtocols)
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state().contains::<IsWebsocket>()
    }

    async fn upgrade(&self, mut upgrade: Upgrade) {
        let peer_ip = upgrade.state.take::<WebsocketPeerIp>().and_then(|i| i.0);
        let mut conn = WebSocketConn::new(upgrade, self.config, Role::Server).await;
        conn.set_peer_ip(peer_ip);

        let Some((mut conn, outbound)) = self.handler.connect(conn).await else {
            return;
        };

        let inbound = conn.take_inbound_stream();

        let mut stream = std::pin::pin!(BidirectionalStream { inbound, outbound });
        while let Some(message) = stream.next().await {
            match message {
                Direction::Inbound(Ok(Message::Close(close_frame))) => {
                    self.handler.disconnect(&mut conn, close_frame).await;
                    break;
                }

                Direction::Inbound(Ok(message)) => {
                    self.handler.inbound(message, &mut conn).await;
                }

                Direction::Outbound(message) => {
                    if let Err(e) = self.handler.send(message, &mut conn).await {
                        log::warn!("outbound websocket error: {:?}", e);
                        break;
                    }
                }

                _ => {
                    self.handler.disconnect(&mut conn, None).await;
                    break;
                }
            }
        }

        if let Some(err) = conn.close().await.err() {
            log::warn!("websocket close error: {:?}", err);
        };
    }
}

fn websocket_protocol(conn: &Conn, protocols: &[String]) -> Option<String> {
    conn.request_headers()
        .get_str(SecWebsocketProtocol)
        .and_then(|value| {
            value
                .split(',')
                .map(str::trim)
                .find(|req_p| protocols.iter().any(|x| x == req_p))
                .map(|s| s.to_owned())
        })
}

fn connection_is_upgrade(conn: &Conn) -> bool {
    conn.request_headers()
        .get_str(Connection)
        .map(|connection| {
            connection
                .split(',')
                .any(|c| c.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
}

fn upgrade_to_websocket(conn: &Conn) -> bool {
    conn.request_headers()
        .eq_ignore_ascii_case(UpgradeHeader, "websocket")
}

fn upgrade_requested(conn: &Conn) -> bool {
    connection_is_upgrade(conn) && upgrade_to_websocket(conn)
}

/// Generate a random key suitable for Sec-WebSocket-Key
pub fn websocket_key() -> String {
    BASE64.encode(fastrand::u128(..).to_ne_bytes())
}

/// Generate the expected Sec-WebSocket-Accept hash from the Sec-WebSocket-Key
pub fn websocket_accept_hash(websocket_key: &str) -> String {
    let hash = Sha1::new()
        .chain_update(websocket_key)
        .chain_update(WEBSOCKET_GUID)
        .finalize();
    BASE64.encode(&hash[..])
}
