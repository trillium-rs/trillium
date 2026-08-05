//! # Trillium tools for server sent events
//!
//! This primarily provides [`SseConnExt`], an
//! extension trait for [`trillium::Conn`] that has a
//! [`with_sse_stream`](crate::SseConnExt::with_sse_stream) chainable
//! method that takes a [`Stream`] where the `Item`
//! implements [`Eventable`].
//!
//! Often, you will want this stream to be something like a channel, but
//! the specifics of that are dependent on the event fanout
//! characteristics of your application.
//!
//! This crate implements [`Eventable`] for an [`Event`] type that you can
//! use in your application, for `String`, and for `&'static str`. You can
//! also implement [`Eventable`] for any type in your application.
//!
//! In addition to data events, the stream can carry comments — messages that clients ignore,
//! used to keep an idle connection from being closed by intermediaries. See
//! [`Event::new_comment`].
//!
//! ## Example usage
//!
//! ```
//! use broadcaster::BroadcastChannel;
//! use trillium::{Conn, Method, State, conn_try, conn_unwrap, log_error};
//! use trillium_sse::SseConnExt;
//! use trillium_static_compiled::static_compiled;
//!
//! type Channel = BroadcastChannel<String>;
//!
//! fn get_sse(mut conn: Conn) -> Conn {
//!     let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
//!     conn.with_sse_stream(broadcaster)
//! }
//!
//! async fn post_broadcast(mut conn: Conn) -> Conn {
//!     let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
//!     let body = conn_try!(conn.request_body_string().await, conn);
//!     log_error!(broadcaster.send(&body).await);
//!     conn.ok("sent")
//! }
//!
//! fn main() {
//!     let handler = (
//!         static_compiled!("examples/static").with_index_file("index.html"),
//!         State::new(Channel::new()),
//!         |conn: Conn| async move {
//!             match (conn.method(), conn.path()) {
//!                 (Method::Get, "/sse") => get_sse(conn),
//!                 (Method::Post, "/broadcast") => post_broadcast(conn).await,
//!                 _ => conn,
//!             }
//!         },
//!     );
//!
//!     // trillium_smol::run(handler);
//! }
//! ```
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs)]

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use futures_lite::{AsyncRead, stream::Stream};
use std::{
    borrow::Cow,
    fmt::Write,
    io,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use trillium::{Body, Conn, KnownHeaderName, Status};

struct SseBody<S, E> {
    stream: S,
    buffer: Vec<u8>,
    event: PhantomData<E>,
}

impl<S, E> SseBody<S, E>
where
    S: Stream<Item = E> + Unpin + Send + Sync + 'static,
    E: Eventable,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            event: PhantomData,
        }
    }
}

fn write_multiline_field(output: &mut String, prefix: &str, value: &str) {
    for line in value.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            writeln!(output, "{prefix}").unwrap();
        } else {
            writeln!(output, "{prefix} {line}").unwrap();
        }
    }
}

/// Returns `None` for an event with no fields set at all, which would otherwise be written as a
/// bare message terminator.
fn encode(event: &impl Eventable) -> Option<String> {
    let mut output = String::new();

    if let Some(comment) = event.comment() {
        write_multiline_field(&mut output, ":", comment);
    }

    if let Some(event_type) = event.event_type() {
        writeln!(&mut output, "event: {event_type}").unwrap();
    }

    if let Some(id) = event.id() {
        writeln!(&mut output, "id: {id}").unwrap();
    }

    if let Some(retry) = event.retry() {
        writeln!(&mut output, "retry: {}", retry.as_millis()).unwrap();
    }

    if let Some(data) = event.data() {
        write_multiline_field(&mut output, "data:", data);
    }

    if output.is_empty() {
        return None;
    }

    writeln!(output).unwrap();

    Some(output)
}

impl<S, E> AsyncRead for SseBody<S, E>
where
    S: Stream<Item = E> + Unpin + Send + Sync + 'static,
    E: Eventable,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let Self { buffer, stream, .. } = self.get_mut();

        let buffer_read = buffer.len().min(buf.len());
        if buffer_read > 0 {
            buf[0..buffer_read].copy_from_slice(&buffer[0..buffer_read]);
            buffer.drain(0..buffer_read);
            return Poll::Ready(Ok(buffer_read));
        }

        loop {
            break match Pin::new(&mut *stream).poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(item)) => {
                    let Some(data) = encode(&item) else { continue };
                    let data = data.into_bytes();
                    let writable_len = data.len().min(buf.len());
                    buf[0..writable_len].copy_from_slice(&data[0..writable_len]);
                    if writable_len < data.len() {
                        buffer.extend_from_slice(&data[writable_len..]);
                    }
                    Poll::Ready(Ok(writable_len))
                }

                Poll::Ready(None) => Poll::Ready(Ok(0)),
            };
        }
    }
}

impl<S, E> From<SseBody<S, E>> for Body
where
    S: Stream<Item = E> + Unpin + Send + Sync + 'static,
    E: Eventable,
{
    fn from(sse_body: SseBody<S, E>) -> Self {
        Body::new_streaming(sse_body, None)
    }
}

/// Extension trait for server sent events
pub trait SseConnExt {
    /// builds and sets a streaming response body that conforms to the
    /// [server-sent-events
    /// spec](https://html.spec.whatwg.org/multipage/server-sent-events.html#server-sent-events)
    /// from a Stream of any [`Eventable`] type (such as
    /// [`Event`], as well as setting appropiate headers for
    /// this response.
    fn with_sse_stream<S, E>(self, sse_stream: S) -> Self
    where
        S: Stream<Item = E> + Unpin + Send + Sync + 'static,
        E: Eventable;
}

impl SseConnExt for Conn {
    fn with_sse_stream<S, E>(self, sse_stream: S) -> Self
    where
        S: Stream<Item = E> + Unpin + Send + Sync + 'static,
        E: Eventable,
    {
        let body = SseBody::new(self.swansong().interrupt(sse_stream));
        self.with_response_header(KnownHeaderName::ContentType, "text/event-stream")
            .with_response_header(KnownHeaderName::CacheControl, "no-cache")
            // Close-delimited framing: the event stream carries neither `Content-Length`
            // nor `Transfer-Encoding`, running until the connection closes. Chunked
            // transfer-encoding can disrupt event delivery timing for this protocol.
            .with_response_header(KnownHeaderName::Connection, "close")
            .with_body(body)
            .with_status(Status::Ok)
            .halt()
    }
}

/// A trait that allows any Unpin + Send + Sync type to act as an event.
///
/// For a concrete implementation of this trait, you can use [`Event`],
/// but it is also implemented for [`String`] and [`&'static str`].
pub trait Eventable: Unpin + Send + Sync + 'static {
    /// return the data for this event, if any
    ///
    /// Returning `None` yields a message with no `data:` field. Clients dispatch no event for
    /// such a message, so it is only useful in combination with [`comment`](Eventable::comment).
    fn data(&self) -> Option<&str>;

    /// return a comment to send alongside this event, optionally
    ///
    /// Comments are ignored by clients. They are chiefly used as a keep-alive, to prevent
    /// intermediaries from closing an otherwise idle event stream.
    fn comment(&self) -> Option<&str> {
        None
    }

    /// return the event type, optionally
    fn event_type(&self) -> Option<&str> {
        None
    }

    /// return a unique event id, optionally
    fn id(&self) -> Option<&str> {
        None
    }

    /// return a reconnection time to request of the client, optionally
    ///
    /// Sent as a `retry:` field in milliseconds, truncated. Clients that reconnect on their own
    /// — such as the browser `EventSource` — wait this long before doing so. Whether and how it
    /// is honored is entirely up to the client.
    fn retry(&self) -> Option<Duration> {
        None
    }
}

impl Eventable for Event {
    fn data(&self) -> Option<&str> {
        Event::data(self)
    }

    fn comment(&self) -> Option<&str> {
        Event::comment(self)
    }

    fn event_type(&self) -> Option<&str> {
        Event::event_type(self)
    }

    fn id(&self) -> Option<&str> {
        Event::id(self)
    }

    fn retry(&self) -> Option<Duration> {
        Event::retry(self)
    }
}

impl Eventable for &'static str {
    fn data(&self) -> Option<&str> {
        Some(self)
    }
}

impl Eventable for String {
    fn data(&self) -> Option<&str> {
        Some(self)
    }
}

/// Events are a concrete implementation of the [`Eventable`] trait.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct Event {
    data: Option<Cow<'static, str>>,
    comment: Option<Cow<'static, str>>,
    event_type: Option<Cow<'static, str>>,
    id: Option<Cow<'static, str>>,
    retry: Option<Duration>,
}

impl From<&'static str> for Event {
    fn from(s: &'static str) -> Self {
        Self::from(Cow::Borrowed(s))
    }
}

impl From<String> for Event {
    fn from(s: String) -> Self {
        Self::from(Cow::Owned(s))
    }
}

impl From<Cow<'static, str>> for Event {
    fn from(data: Cow<'static, str>) -> Self {
        Event {
            data: Some(data),
            ..Self::default()
        }
    }
}

impl Event {
    /// builds a new [`Event`]
    ///
    /// by default, this event has no event type. to set an event type,
    /// use [`Event::with_type`] or [`Event::set_type`]
    pub fn new(data: impl Into<Cow<'static, str>>) -> Self {
        Self::from(data.into())
    }

    /// builds a new comment-only [`Event`], with no data
    ///
    /// Clients ignore comments and dispatch no event for this message. Sending one periodically
    /// keeps an otherwise idle event stream from being closed by intermediaries.
    ///
    /// ```
    /// let event = trillium_sse::Event::new_comment("keep-alive");
    /// assert_eq!(event.comment(), Some("keep-alive"));
    /// assert_eq!(event.data(), None);
    /// ```
    pub fn new_comment(comment: impl Into<Cow<'static, str>>) -> Self {
        Self {
            comment: Some(comment.into()),
            ..Self::default()
        }
    }

    /// chainable constructor to set the type on an event
    ///
    /// ```
    /// let event = trillium_sse::Event::new("event data").with_type("userdata");
    /// assert_eq!(event.event_type(), Some("userdata"));
    /// assert_eq!(event.data(), Some("event data"));
    /// ```
    pub fn with_type(mut self, event_type: impl Into<Cow<'static, str>>) -> Self {
        self.set_type(event_type);
        self
    }

    /// set the event type for this Event. The default is None.
    ///
    /// ```
    /// let mut event = trillium_sse::Event::new("event data");
    /// assert_eq!(event.event_type(), None);
    /// event.set_type("userdata");
    /// assert_eq!(event.event_type(), Some("userdata"));
    /// ```
    pub fn set_type(&mut self, event_type: impl Into<Cow<'static, str>>) {
        self.event_type = Some(event_type.into());
    }

    /// chainable constructor to set the id on an event
    ///
    /// ```
    /// let event = trillium_sse::Event::new("event data").with_id("1");
    /// assert_eq!(event.id(), Some("1"));
    /// ```
    pub fn with_id(mut self, id: impl Into<Cow<'static, str>>) -> Self {
        self.set_id(id);
        self
    }

    /// set the id for this Event. The default is None.
    pub fn set_id(&mut self, id: impl Into<Cow<'static, str>>) {
        self.id = Some(id.into());
    }

    /// chainable constructor to attach a comment to an event
    ///
    /// Comments are ignored by clients, and can be attached to an event with data as well as
    /// sent on their own with [`Event::new_comment`].
    ///
    /// ```
    /// let event = trillium_sse::Event::new("event data").with_comment("ignore me");
    /// assert_eq!(event.comment(), Some("ignore me"));
    /// assert_eq!(event.data(), Some("event data"));
    /// ```
    pub fn with_comment(mut self, comment: impl Into<Cow<'static, str>>) -> Self {
        self.set_comment(comment);
        self
    }

    /// set the comment for this Event. The default is None.
    pub fn set_comment(&mut self, comment: impl Into<Cow<'static, str>>) {
        self.comment = Some(comment.into());
    }

    /// chainable constructor to set the reconnection time on an event
    ///
    /// ```
    /// use std::time::Duration;
    /// let event = trillium_sse::Event::new("event data").with_retry(Duration::from_secs(5));
    /// assert_eq!(event.retry(), Some(Duration::from_secs(5)));
    /// ```
    pub fn with_retry(mut self, retry: Duration) -> Self {
        self.set_retry(retry);
        self
    }

    /// set the reconnection time for this Event. The default is None.
    ///
    /// This is sent to the client as a `retry:` field in milliseconds; see
    /// [`Eventable::retry`]. A retry can be sent without any data, as
    /// `Event::default().with_retry(duration)`.
    pub fn set_retry(&mut self, retry: Duration) {
        self.retry = Some(retry);
    }

    /// returns this Event's data as a str, if set
    pub fn data(&self) -> Option<&str> {
        self.data.as_deref()
    }

    /// returns this Event's comment as a str, if set
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    /// returns this Event's type as a str, if set
    pub fn event_type(&self) -> Option<&str> {
        self.event_type.as_deref()
    }

    /// returns this Event's id as a str, if set
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// returns this Event's reconnection time, if set
    pub fn retry(&self) -> Option<Duration> {
        self.retry
    }
}
