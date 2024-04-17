/*!
# websocket json adapter

See the documentation for [`JsonWebSocketHandler`]
*/

use crate::{WebSocket, WebSocketConn, WebSocketHandler};
use async_tungstenite::tungstenite::{protocol::CloseFrame, Message};
use futures_lite::{ready, Stream};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll},
};
use trillium::async_trait;

/**
# Implement this trait to use websockets with a json handler

JsonWebSocketHandler provides a small layer of abstraction on top of
[`WebSocketHandler`], serializing and deserializing messages for
you. This may eventually move to a crate of its own.

## ℹ️ In order to use this trait, the `json` crate feature must be enabled.

```
use async_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use trillium::{async_trait, log_error};
use trillium_websockets::{json_websocket, JsonWebSocketHandler, WebSocketConn, Result};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Response {
    inbound_message: Inbound,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Inbound {
    message: String,
}

struct SomeJsonChannel;

#[async_trait]
impl JsonWebSocketHandler for SomeJsonChannel {
    type InboundMessage = Inbound;
    type OutboundMessage = Response;
    type StreamType = Pin<Box<Receiver<Self::OutboundMessage>>>;

    async fn connect(&self, conn: &mut WebSocketConn) -> Self::StreamType {
        let (s, r) = unbounded();
        conn.insert_state(s);
        Box::pin(r)
    }

    async fn receive_message(
        &self,
        inbound_message: Result<Self::InboundMessage>,
        conn: &mut WebSocketConn,
    ) {
        if let Ok(inbound_message) = inbound_message {
            log_error!(
                conn.state::<Sender<Response>>()
                    .unwrap()
                    .send(Response { inbound_message })
                    .await
            );
        }
    }
}

// fn main() {
//    trillium_smol::run(json_websocket(SomeJsonChannel));
// }
```

*/
#[allow(unused_variables)]
#[async_trait]
pub trait JsonWebSocketHandler: Send + Sync + 'static {
    /**
    A type that can be deserialized from the json sent from the
    connected clients
    */
    type InboundMessage: DeserializeOwned + Send + 'static;

    /**
    A serializable type that will be sent in the StreamType and
    received by the connected websocket clients
    */
    type OutboundMessage: Serialize + Send + 'static;

    /**
    A type that implements a stream of
    [`Self::OutboundMessage`]s. This can be
    futures_lite::stream::Pending if you never need to send an
    outbound message.
    */
    type StreamType: Stream<Item = Self::OutboundMessage> + Send + Sync + 'static;

    /**
    `connect` is called once for each upgraded websocket
    connection, and returns a Self::StreamType.
    */
    async fn connect(&self, conn: &mut WebSocketConn) -> Self::StreamType;

    /**
    `receive_message` is called once for each successfully deserialized
    InboundMessage along with the websocket conn that it was received
    from.
    */
    async fn receive_message(
        &self,
        message: crate::Result<Self::InboundMessage>,
        conn: &mut WebSocketConn,
    );

    /**
    `disconnect` is called when websocket clients disconnect, along
    with a CloseFrame, if one was provided. Implementing `disconnect`
    is optional.
    */
    async fn disconnect(&self, conn: &mut WebSocketConn, close_frame: Option<CloseFrame<'static>>) {
    }
}

/**
A wrapper type for [`JsonWebSocketHandler`]s

You do not need to interact with this type directly. Instead, use
[`WebSocket::new_json`] or [`json_websocket`].
*/
pub struct JsonHandler<T> {
    pub(crate) handler: T,
}

impl<T> Deref for JsonHandler<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.handler
    }
}

impl<T> DerefMut for JsonHandler<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.handler
    }
}

impl<T: JsonWebSocketHandler> JsonHandler<T> {
    pub(crate) fn new(handler: T) -> Self {
        Self { handler }
    }
}

impl<T> Debug for JsonHandler<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonWebSocketHandler").finish()
    }
}

pin_project_lite::pin_project! {
    /**
    A stream for internal use that attempts to serialize the items in the
    wrapped stream to a [`Message::Text`]
     */
    #[derive(Debug)]
    pub struct SerializedStream<T> {
        #[pin] inner: T
    }
}

impl<T> Stream for SerializedStream<T>
where
    T: Stream,
    T::Item: Serialize,
{
    type Item = Message;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(
            ready!(self.project().inner.poll_next(cx))
                .and_then(|i| match serde_json::to_string(&i) {
                    Ok(j) => Some(j),
                    Err(e) => {
                        log::error!("serialization error: {e}");
                        None
                    }
                })
                .map(Message::Text),
        )
    }
}

#[async_trait]
impl<T> WebSocketHandler for JsonHandler<T>
where
    T: JsonWebSocketHandler,
{
    type OutboundStream = SerializedStream<T::StreamType>;

    async fn connect(
        &self,
        mut conn: WebSocketConn,
    ) -> Option<(WebSocketConn, Self::OutboundStream)> {
        let stream = SerializedStream {
            inner: self.handler.connect(&mut conn).await,
        };
        Some((conn, stream))
    }

    async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
        self.handler
            .receive_message(
                message
                    .to_text()
                    .map_err(Into::into)
                    .and_then(|m| serde_json::from_str(m).map_err(Into::into)),
                conn,
            )
            .await;
    }

    async fn disconnect(&self, conn: &mut WebSocketConn, close_frame: Option<CloseFrame<'static>>) {
        self.handler.disconnect(conn, close_frame).await
    }
}

impl<T> WebSocket<JsonHandler<T>>
where
    T: JsonWebSocketHandler,
{
    /**
    Build a new trillium WebSocket handler from the provided
    [`JsonWebSocketHandler`]
     */
    pub fn new_json(handler: T) -> Self {
        Self::new(JsonHandler::new(handler))
    }
}

/**
builds a new trillium handler from the provided
[`JsonWebSocketHandler`]. Alias for [`WebSocket::new_json`]
*/
pub fn json_websocket<T>(json_websocket_handler: T) -> WebSocket<JsonHandler<T>>
where
    T: JsonWebSocketHandler,
{
    WebSocket::new_json(json_websocket_handler)
}
