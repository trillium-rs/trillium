use crate::WebSocketConn;
use async_tungstenite::tungstenite::{protocol::CloseFrame, Error, Message};
use futures_lite::stream::{Pending, Stream};
use std::future::Future;
use trillium::async_trait;

/**
# This is the trait that defines a handler for trillium websockets.

There are several mutually-exclusive ways to use this trait, and it is
intended to be flexible for different use cases. If the trait does not
support your use case, please open a discussion and/or build a trait
on top of this trait to add additional functionality.

## Simple Example
```
use trillium_websockets::{Message, WebSocket, WebSocketConn, WebSocketHandler};
use futures_lite::stream::{pending, Pending};

struct EchoServer;
#[trillium::async_trait]
impl WebSocketHandler for EchoServer {
    type OutboundStream = Pending<Message>; // we don't use an outbound stream in this example

    async fn connect(&self, conn: WebSocketConn) -> Option<(WebSocketConn, Self::OutboundStream)> {
        Some((conn, pending()))
    }

    async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
        let path = conn.path().to_string();
        if let Message::Text(input) = message {
            let reply = format!("received your message: {} at path {}", &input, &path);
            conn.send_string(reply).await;
        }
    }
}

let handler = WebSocket::new(EchoServer);
# // tests at tests/tests.rs for example simplicity
```


## Using [`WebSocketHandler::connect`] only

If you have needs that are not supported by this trait, you can either
pass an `Fn(WebSocketConn) -> impl Future<Output=()>` as a handler, or
implement your own connect-only trait implementation that takes the
WebSocketConn and returns None. The tcp connection will remain intact
until the WebSocketConn is dropped, so you can store it in any data
structure or move it between threads as needed.

## Using Streams

If you define an associated OutboundStream type and return it from
`connect`, every message in that Stream will be sent to the connected
websocket client. This is useful for sending messages that are
triggered by other events in the application, using whatever channel
mechanism is appropriate for your application. The websocket
connection will be closed if the stream ends, yielding None.

If you do not need to use streams, set `OutboundStream =
futures_lite::stream::Pending<Message>` or a similar stream
implementation that never yields. If associated type defaults were
stable, we would use that.

## Receiving client-sent messages

Implement [`WebSocketHandler::inbound`] to receive client-sent
messages. Currently inbound messages are not represented as a stream,
but this may change in the future.

## Holding data inside of the implementing type

As this is a trait you implement for your own type, you can hold
additional data or structs inside of your struct. There will be
exactly one of these structs shared throughout the application, so
async concurrency types can be used to mutate shared data.

This example holds a shared BroadcastChannel that is cloned for each
OutboundStream. Any message that a connected clients sends is
broadcast to every other connected client.

Importantly, this means that the dispatch and fanout of messages is
managed entirely by your implementation. For an opinionated layer on
top of this, see the trillium-channels crate.

```
use broadcaster::BroadcastChannel;
use trillium_websockets::{Message, WebSocket, WebSocketConn, WebSocketHandler};

struct EchoServer {
    channel: BroadcastChannel<Message>,
}
impl EchoServer {
    fn new() -> Self {
        Self {
            channel: BroadcastChannel::new(),
        }
    }
}

#[trillium::async_trait]
impl WebSocketHandler for EchoServer {
    type OutboundStream = BroadcastChannel<Message>;

    async fn connect(&self, conn: WebSocketConn) -> Option<(WebSocketConn, Self::OutboundStream)> {
        Some((conn, self.channel.clone()))
    }

    async fn inbound(&self, message: Message, _conn: &mut WebSocketConn) {
        if let Message::Text(input) = message {
            let message = Message::text(format!("received message: {}", &input));
            trillium::log_error!(self.channel.send(&message).await);
        }
    }
}

// fn main() {
//     trillium_smol::run(WebSocket::new(EchoServer::new()));
// }

```

*/
#[allow(unused_variables)]
#[async_trait]
pub trait WebSocketHandler: Send + Sync + Sized + 'static {
    /**
    A [`Stream`] type that represents [`Message`]s to be sent to this
    client. It is built in your implementation code, in
    [`WebSocketHandler::connect`]. Use `Pending<Message>` or another
    stream that never returns if you do not need to use this aspect of
    the trait.
    */
    type OutboundStream: Stream<Item = Message> + Unpin + Send + Sync + 'static;

    /**
    This interface is the only mandatory function in
    WebSocketHandler. It receives an owned WebSocketConn and
    optionally returns it along with an `OutboundStream`
    type.
     */
    async fn connect(&self, conn: WebSocketConn) -> Option<(WebSocketConn, Self::OutboundStream)>;

    /**
    This interface function is called once with every message received
    from a connected websocket client.
    */
    async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {}

    /**
    This interface function is called once with every outbound message
    in the OutboundStream. You likely do not need to implement this,
    but if you do, you must call `conn.send(message).await` or the
    message will not be sent.
    */
    async fn send(&self, message: Message, conn: &mut WebSocketConn) -> Result<(), Error> {
        conn.send(message).await
    }

    /**
    This interface function is called with the websocket conn and, in
    the case of a clean disconnect, the [`CloseFrame`] if one is sent
    available.
    */
    async fn disconnect(&self, conn: &mut WebSocketConn, close_frame: Option<CloseFrame<'static>>) {
    }
}

#[async_trait]
impl<H, Fut> WebSocketHandler for H
where
    H: Fn(WebSocketConn) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    type OutboundStream = Pending<Message>;

    async fn connect(&self, wsc: WebSocketConn) -> Option<(WebSocketConn, Self::OutboundStream)> {
        self(wsc).await;

        None
    }
}
