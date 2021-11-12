use crate::Result;
use async_tungstenite::{
    tungstenite::{protocol::Role, Message},
    WebSocketStream,
};
use futures_util::{
    stream::{SplitSink, SplitStream, Stream},
    SinkExt, StreamExt,
};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use stopper::{Stopper, StreamStopper};
use trillium::{Headers, Method, StateSet, Upgrade};
use trillium_http::transport::BoxedTransport;

/**
A struct that represents an specific websocket connection.

This can be thought of as a combination of a [`async_tungstenite::WebSocketStream`] and a
[`trillium::Conn`], as it contains a combination of their fields and
associated functions.

The WebSocketConn implements `Stream<Item=Result<Message, Error>>`,
and can be polled with `StreamExt::next`
 */

#[derive(Debug)]
pub struct WebSocketConn {
    request_headers: Headers,
    path: String,
    method: Method,
    state: StateSet,
    stopper: Stopper,
    sink: SplitSink<Wss, Message>,
    stream: Option<WStream>,
}

type Wss = WebSocketStream<BoxedTransport>;

impl WebSocketConn {
    /// send a [`Message::Text`] variant
    pub async fn send_string(&mut self, string: String) {
        self.send(Message::Text(string)).await.ok();
    }

    /// send a [`Message::Binary`] variant
    pub async fn send_bytes(&mut self, bin: Vec<u8>) {
        self.send(Message::Binary(bin)).await.ok();
    }

    #[cfg(feature = "json")]
    /// send a [`Message::Text`] that contains json
    /// note that json messages are not actually part of the websocket specification
    pub async fn send_json(&mut self, json: &impl serde::Serialize) -> serde_json::Result<()> {
        self.send_string(serde_json::to_string(json)?).await;
        Ok(())
    }

    /// Sends a [`Message`] to the client
    pub async fn send(&mut self, message: Message) -> async_tungstenite::tungstenite::Result<()> {
        self.sink.send(message).await
    }

    pub(crate) async fn new(upgrade: Upgrade) -> Self {
        let Upgrade {
            request_headers,
            path,
            method,
            state,
            buffer,
            transport,
            stopper,
        } = upgrade;

        let wss = if let Some(vec) = buffer {
            WebSocketStream::from_partially_read(transport, vec, Role::Server, None).await
        } else {
            WebSocketStream::from_raw_socket(transport, Role::Server, None).await
        };

        let (sink, stream) = wss.split();
        let stream = Some(WStream {
            stream: stopper.stop_stream(stream),
        });

        Self {
            request_headers,
            path,
            method,
            state,
            sink,
            stream,
            stopper,
        }
    }

    /// retrieve a clone of the server's [`Stopper`]
    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    /// close the websocket connection gracefully
    pub async fn close(&mut self) -> async_tungstenite::tungstenite::Result<()> {
        self.send(Message::Close(None)).await
    }

    /// retrieve the request headers for this conn
    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }

    /**
    retrieves the path part of the request url, up to and excluding
    any query component
     */
    pub fn path(&self) -> &str {
        self.path.split('?').next().unwrap()
    }

    /**
    Retrieves the query component of the path, excluding `?`. Returns
    an empty string if there is no query component.
     */
    pub fn querystring(&self) -> &str {
        match self.path.split_once('?') {
            Some((_, query)) => query,
            None => "",
        }
    }

    /// retrieve the request method for this conn
    pub fn method(&self) -> Method {
        self.method
    }

    /**
    retrieve state from the state set that has been accumulated by
    trillium handlers run on the [`trillium::Conn`] before it
    became a websocket. see [`trillium::Conn::state`] for more
    information
     */
    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.state.get()
    }

    /**
    retrieve a mutable borrow of the state from the state set
     */
    pub fn state_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.state.get_mut()
    }

    /**
    set state on this connection
    */
    pub fn set_state<T: Send + Sync + 'static>(&mut self, val: T) {
        self.state.insert(val);
    }

    /**
    take some type T out of the state set that has been
    accumulated by trillium handlers run on the [`trillium::Conn`]
    before it became a websocket. see [`trillium::Conn::take_state`]
    for more information
     */
    pub fn take_state<T: 'static>(&mut self) -> Option<T> {
        self.state.take()
    }

    /// take the inbound Message stream from this conn
    pub fn take_inbound_stream(&mut self) -> Option<impl Stream<Item = Result>> {
        self.stream.take()
    }

    /// borrow the inbound Message stream from this conn
    pub fn inbound_stream(&mut self) -> Option<impl Stream<Item = Result> + '_> {
        self.stream.as_mut()
    }
}

#[derive(Debug)]
pub struct WStream {
    stream: StreamStopper<SplitStream<Wss>>,
}

impl Stream for WStream {
    type Item = Result;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}

impl AsMut<StateSet> for WebSocketConn {
    fn as_mut(&mut self) -> &mut StateSet {
        &mut self.state
    }
}

impl AsRef<StateSet> for WebSocketConn {
    fn as_ref(&self) -> &StateSet {
        &self.state
    }
}

impl Stream for WebSocketConn {
    type Item = Result;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.stream.as_mut() {
            Some(stream) => stream.poll_next_unpin(cx),
            None => Poll::Ready(None),
        }
    }
}
