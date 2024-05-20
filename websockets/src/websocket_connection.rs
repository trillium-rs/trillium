use crate::{Result, Role, WebSocketConfig};
use async_tungstenite::{
    tungstenite::{self, Message},
    WebSocketStream,
};
use futures_util::{
    stream::{SplitSink, SplitStream, Stream},
    SinkExt, StreamExt,
};
use std::{
    net::IpAddr,
    pin::Pin,
    task::{Context, Poll},
};
use swansong::{Interrupt, Swansong};
use trillium::{Headers, Method, TypeSet, Upgrade};
use trillium_http::{transport::BoxedTransport, type_set::entry::Entry};

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
    state: TypeSet,
    peer_ip: Option<IpAddr>,
    swansong: Swansong,
    sink: SplitSink<Wss, Message>,
    stream: Option<WStream>,
}

type Wss = WebSocketStream<BoxedTransport>;

impl WebSocketConn {
    /// send a [`Message::Text`] variant
    pub async fn send_string(&mut self, string: String) -> Result<()> {
        self.send(Message::Text(string)).await.map_err(Into::into)
    }

    /// send a [`Message::Binary`] variant
    pub async fn send_bytes(&mut self, bin: Vec<u8>) -> Result<()> {
        self.send(Message::Binary(bin)).await.map_err(Into::into)
    }

    #[cfg(feature = "json")]
    /// send a [`Message::Text`] that contains json
    /// note that json messages are not actually part of the websocket specification
    pub async fn send_json(&mut self, json: &impl serde::Serialize) -> Result<()> {
        self.send_string(serde_json::to_string(json)?).await
    }

    /// Sends a [`Message`] to the client
    pub async fn send(&mut self, message: Message) -> Result<()> {
        self.sink.send(message).await.map_err(Into::into)
    }

    /// Create a `WebSocketConn` from an HTTP upgrade, with optional config and the specified role
    ///
    /// You should not typically need to call this; the trillium client and server both provide
    /// your code with a `WebSocketConn`.
    #[doc(hidden)]
    pub async fn new(upgrade: Upgrade, config: Option<WebSocketConfig>, role: Role) -> Self {
        let Upgrade {
            request_headers,
            path,
            method,
            state,
            buffer,
            transport,
            swansong,
            peer_ip,
            ..
        } = upgrade;

        let wss = if buffer.is_empty() {
            WebSocketStream::from_raw_socket(transport, role, config).await
        } else {
            WebSocketStream::from_partially_read(transport, buffer.to_owned(), role, config).await
        };

        let (sink, stream) = wss.split();
        let stream = Some(WStream {
            stream: swansong.interrupt(stream),
        });

        Self {
            request_headers,
            path,
            method,
            state,
            peer_ip,
            sink,
            stream,
            swansong,
        }
    }

    /// retrieve a clone of the server's [`Swansong`]
    pub fn swansong(&self) -> Swansong {
        self.swansong.clone()
    }

    /// close the websocket connection gracefully
    pub async fn close(&mut self) -> Result<()> {
        self.send(Message::Close(None)).await
    }

    /// retrieve the request headers for this conn
    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }

    /// retrieves the peer ip for this conn, if available
    pub fn peer_ip(&self) -> Option<IpAddr> {
        self.peer_ip
    }

    /// Sets the peer ip for this conn
    pub fn set_peer_ip(&mut self, peer_ip: Option<IpAddr>) {
        self.peer_ip = peer_ip
    }

    /**
    retrieves the path part of the request url, up to and excluding
    any query component
     */
    pub fn path(&self) -> &str {
        self.path.split('?').next().unwrap_or_default()
    }

    /**
    Retrieves the query component of the path, excluding `?`. Returns
    an empty string if there is no query component.
     */
    pub fn querystring(&self) -> &str {
        self.path
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or_default()
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
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.state.get()
    }

    /**
    retrieve a mutable borrow of the state from the state set
     */
    pub fn state_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.state.get_mut()
    }

    /// inserts new state
    ///
    /// returns the previously set state of the same type, if any existed
    pub fn insert_state<T: Send + Sync + 'static>(&mut self, state: T) -> Option<T> {
        self.state.insert(state)
    }

    /**
    take some type T out of the state set that has been
    accumulated by trillium handlers run on the [`trillium::Conn`]
    before it became a websocket. see [`trillium::Conn::take_state`]
    for more information
     */
    pub fn take_state<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.state.take()
    }

    /// Returns an [`Entry`] for the state typeset that can be used with functions like
    /// [`Entry::or_insert`], [`Entry::or_insert_with`], [`Entry::and_modify`], and others.
    pub fn state_entry<T: Send + Sync + 'static>(&mut self) -> Entry<'_, T> {
        self.state.entry()
    }

    /// take the inbound Message stream from this conn
    pub fn take_inbound_stream(&mut self) -> Option<impl Stream<Item = MessageResult>> {
        self.stream.take()
    }

    /// borrow the inbound Message stream from this conn
    pub fn inbound_stream(&mut self) -> Option<impl Stream<Item = MessageResult> + '_> {
        self.stream.as_mut()
    }
}

type MessageResult = std::result::Result<Message, tungstenite::Error>;

#[derive(Debug)]
pub struct WStream {
    stream: Interrupt<SplitStream<Wss>>,
}

impl Stream for WStream {
    type Item = MessageResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}

impl AsMut<TypeSet> for WebSocketConn {
    fn as_mut(&mut self) -> &mut TypeSet {
        &mut self.state
    }
}

impl AsRef<TypeSet> for WebSocketConn {
    fn as_ref(&self) -> &TypeSet {
        &self.state
    }
}

impl Stream for WebSocketConn {
    type Item = MessageResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.stream.as_mut() {
            Some(stream) => stream.poll_next_unpin(cx),
            None => Poll::Ready(None),
        }
    }
}
