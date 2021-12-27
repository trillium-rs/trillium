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

use bidirectional_stream::{BidirectionalStream, Direction};
use futures_lite::stream::StreamExt;
use sha1::{Digest, Sha1};
use std::ops::{Deref, DerefMut};
use trillium::{
    async_trait, conn_unwrap, log_error, Conn, Handler,
    KnownHeaderName::{
        Connection, SecWebsocketAccept, SecWebsocketKey, SecWebsocketProtocol, SecWebsocketVersion,
        Upgrade as UpgradeHeader,
    },
    Status, Upgrade,
};

pub use async_tungstenite::{
    self,
    tungstenite::{self, Error, Message},
};
pub use websocket_connection::WebSocketConn;
pub use websocket_handler::WebSocketHandler;

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// a Result type for websocket messages
pub type Result = std::result::Result<Message, Error>;

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
}

struct IsWebsocket;

#[cfg(test)]
mod tests;

macro_rules! unwrap_or_return {
    ($expr:expr) => {
        match $expr {
            Some(x) => x,
            None => return,
        }
    };
}

#[async_trait]
impl<H> Handler for WebSocket<H>
where
    H: WebSocketHandler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        if !upgrade_requested(&conn) {
            return conn;
        }

        let sec_websocket_accept = conn_unwrap!(
            websocket_accept_hash(&conn),
            conn.with_status(Status::BadRequest)
        );

        let protocol = websocket_protocol(&conn, &self.protocols);

        let headers = conn.headers_mut();

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
            .with_state(IsWebsocket)
            .with_status(Status::SwitchingProtocols)
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state().contains::<IsWebsocket>()
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        let conn = WebSocketConn::new(upgrade).await;
        let (mut conn, outbound) = unwrap_or_return!(self.handler.connect(conn).await);
        let inbound = conn.take_inbound_stream();

        let mut stream = BidirectionalStream { inbound, outbound };
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
    conn.headers()
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
    conn.headers()
        .get_str(Connection)
        .map(|connection| {
            connection
                .split(',')
                .any(|c| c.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
}

fn upgrade_to_websocket(conn: &Conn) -> bool {
    conn.headers()
        .eq_ignore_ascii_case(UpgradeHeader, "websocket")
}

fn upgrade_requested(conn: &Conn) -> bool {
    connection_is_upgrade(conn) && upgrade_to_websocket(conn)
}

fn websocket_accept_hash(conn: &Conn) -> Option<String> {
    let websocket_key = conn.headers().get_str(SecWebsocketKey)?;

    let hash = Sha1::new()
        .chain_update(websocket_key)
        .chain_update(WEBSOCKET_GUID)
        .finalize();

    Some(base64::encode(&hash[..]))
}
