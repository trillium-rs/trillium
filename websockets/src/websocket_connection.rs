use crate::{Result, Role, WebSocketConfig};
use async_tungstenite::{
    WebSocketReceiver, WebSocketSender, WebSocketStream,
    tungstenite::{self, Message},
};
use futures_lite::{Stream, StreamExt, future};
use futures_sink::Sink;
use std::{
    borrow::Cow,
    fmt::Debug,
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    task::{self, Poll},
};
use swansong::{Interrupt, Swansong};
use trillium::{Headers, Method, Transport, TypeSet, Upgrade};
use trillium_http::{HttpContext, type_set::entry::Entry};

/// A struct that represents an specific websocket connection.
///
/// This can be thought of as a combination of a [`async_tungstenite::WebSocketStream`] and a
/// [`trillium::Conn`], as it contains a combination of their fields and
/// associated functions.
///
/// The WebSocketConn implements `Stream<Item=Result<Message, Error>>`,
/// and can be polled with `StreamExt::next`
pub struct WebSocketConn {
    request_headers: Headers,
    path: Cow<'static, str>,
    querystring: Cow<'static, str>,
    method: Method,
    state: TypeSet,
    peer_ip: Option<IpAddr>,
    context: Arc<HttpContext>,
    sink: WebSocketSender<Box<dyn Transport>>,
    stream: Option<WStream>,
}

impl Debug for WebSocketConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketConn")
            .field("request_headers", &self.request_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("state", &self.state)
            .field("peer_ip", &self.peer_ip)
            .field("context", &self.context)
            .field("stream", &self.stream)
            .finish_non_exhaustive()
    }
}

impl WebSocketConn {
    /// send a [`Message::Text`] variant
    pub async fn send_string(&mut self, string: String) -> Result<()> {
        self.send(Message::text(string)).await
    }

    /// send a [`Message::Binary`] variant
    pub async fn send_bytes(&mut self, bin: Vec<u8>) -> Result<()> {
        self.send(Message::binary(bin)).await
    }

    #[cfg(feature = "json")]
    /// send a [`Message::Text`] that contains json
    /// note that json messages are not actually part of the websocket specification
    pub async fn send_json(&mut self, json: &impl serde::Serialize) -> Result<()> {
        self.send_string(serde_json::to_string(json)?).await
    }

    /// Sends a [`Message`] to the client and flushes it to the socket
    ///
    /// When sending many messages in quick succession, [`feed`][Self::feed] coalesces them into
    /// fewer socket writes.
    pub async fn send(&mut self, message: Message) -> Result<()> {
        self.feed(message).await?;
        self.flush().await
    }

    /// Enqueues a [`Message`] without immediately writing it to the socket
    ///
    /// The message is encoded into an internal write buffer. The buffer is written out when it
    /// fills, when this conn is next polled for an inbound message and none is immediately
    /// available, or on [`flush`][Self::flush] or [`send`][Self::send]. If you feed a message and
    /// then await anything other than this conn, call `flush` first.
    pub async fn feed(&mut self, message: Message) -> Result<()> {
        future::poll_fn(|cx| Pin::new(&mut self.sink).poll_ready(cx)).await?;
        Pin::new(&mut self.sink).start_send(message)?;
        Ok(())
    }

    /// Writes any buffered outbound messages to the socket
    pub async fn flush(&mut self) -> Result<()> {
        future::poll_fn(|cx| Pin::new(&mut self.sink).poll_flush(cx))
            .await
            .map_err(Into::into)
    }

    /// Create a `WebSocketConn` from an HTTP upgrade, with optional config and the specified role
    ///
    /// You should not typically need to call this; the trillium client and server both provide
    /// your code with a `WebSocketConn`.
    #[doc(hidden)]
    pub async fn new(
        upgrade: impl Into<Upgrade>,
        config: Option<WebSocketConfig>,
        role: Role,
    ) -> Self {
        let mut upgrade = upgrade.into();
        let request_headers = upgrade.take_request_headers();
        let path = upgrade.path().to_string().into();
        let path = upgrade.querystring().to_string().into();
        let method = upgrade.method();
        let state = upgrade.take_state();
        let context = upgrade.context().clone();
        let peer_ip = upgrade.peer_ip();
        let (buffer, transport) = upgrade.into_transport();

        let wss = if buffer.is_empty() {
            WebSocketStream::from_raw_socket(transport, role, config).await
        } else {
            WebSocketStream::from_partially_read(transport, buffer, role, config).await
        };

        let (sink, stream) = wss.split();
        let stream = Some(WStream {
            stream: context.swansong().interrupt(stream),
        });

        Self {
            request_headers,
            path,
            querystring,
            method,
            state,
            peer_ip,
            sink,
            stream,
            context,
        }
    }

    /// retrieve a clone of the server's [`Swansong`]
    pub fn swansong(&self) -> Swansong {
        self.context.swansong().clone()
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
    pub fn set_peer_ip(&mut self, peer_ip: Option<IpAddr>) -> &mut Self {
        self.peer_ip = peer_ip;
        self
    }

    /// retrieves the path part of the request url, up to and excluding
    /// any query component
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Retrieves the query component of the path, excluding `?`. Returns
    /// an empty string if there is no query component.
    pub fn querystring(&self) -> &str {
        &self.querystring
    }

    /// retrieve the request method for this conn
    pub fn method(&self) -> Method {
        self.method
    }

    /// retrieve state from the state set that has been accumulated by
    /// trillium handlers run on the [`trillium::Conn`] before it
    /// became a websocket. see [`trillium::Conn::state`] for more
    /// information
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.state.get()
    }

    /// retrieve a mutable borrow of the state from the state set
    pub fn state_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.state.get_mut()
    }

    /// inserts new state
    ///
    /// returns the previously set state of the same type, if any existed
    pub fn insert_state<T: Send + Sync + 'static>(&mut self, state: T) -> Option<T> {
        self.state.insert(state)
    }

    /// Returns an [`Entry`] for the state typeset that can be used with functions like
    /// [`Entry::or_insert`], [`Entry::or_insert_with`], [`Entry::and_modify`], and others.
    pub fn state_entry<T: Send + Sync + 'static>(&mut self) -> Entry<'_, T> {
        self.state.entry()
    }

    /// take some type T out of the state set that has been
    /// accumulated by trillium handlers run on the [`trillium::Conn`]
    /// before it became a websocket. see [`trillium::Conn::take_state`]
    /// for more information
    pub fn take_state<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.state.take()
    }

    pub(crate) fn poll_flush_sink(
        &mut self,
        cx: &mut task::Context<'_>,
    ) -> Poll<std::result::Result<(), tungstenite::Error>> {
        Pin::new(&mut self.sink).poll_flush(cx)
    }

    /// take the inbound Message stream from this conn
    pub fn take_inbound_stream(&mut self) -> Option<impl Stream<Item = MessageResult> + use<>> {
        self.stream.take()
    }

    /// borrow the inbound Message stream from this conn
    pub fn inbound_stream(&mut self) -> Option<impl Stream<Item = MessageResult> + '_> {
        self.stream.as_mut()
    }
}

type MessageResult = std::result::Result<Message, tungstenite::Error>;

pub struct WStream {
    stream: Interrupt<WebSocketReceiver<Box<dyn Transport>>>,
}

impl Debug for WStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WStream").finish_non_exhaustive()
    }
}

impl Stream for WStream {
    type Item = MessageResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next(cx)
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

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let poll = match this.stream.as_mut() {
            Some(stream) => Pin::new(stream).poll_next(cx),
            None => Poll::Ready(None),
        };

        // About to yield to the caller with nothing to process — write out anything `send`
        // buffered. Errors are deliberately dropped here; they resurface on the next send or
        // flush, or as stream termination.
        if !matches!(poll, Poll::Ready(Some(_)))
            && let Poll::Ready(Err(e)) = this.poll_flush_sink(cx)
        {
            log::debug!("websocket flush error: {e}");
        }

        poll
    }
}
