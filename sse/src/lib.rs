//! # Trillium tools for server sent events
//!
//! There are two ways to use this crate.
//!
//! ## The [`Sse`] handler
//!
//! [`sse`] builds a [`Handler`](trillium::Handler) from any [`SseHandler`], which produces a
//! [`Stream`] of [`Eventable`] items per connected client. This is the fuller-featured of the
//! two, because a handler is initialized by the server and so can send heartbeats.
//!
//! Requests whose `Accept` header excludes `text/event-stream` are passed through to subsequent
//! handlers untouched.
//!
//! ```
//! use broadcaster::BroadcastChannel;
//! use std::time::Duration;
//! use trillium::Conn;
//! use trillium_sse::sse;
//!
//! let channel = BroadcastChannel::<String>::new();
//!
//! let handler = sse(move |_: &mut Conn| channel.clone()).with_heartbeat(Duration::from_secs(15));
//! ```
//!
//! ## [`SseConnExt`]
//!
//! [`SseConnExt`] is an extension trait for [`trillium::Conn`] whose
//! [`with_sse_stream`](crate::SseConnExt::with_sse_stream) chainable method takes a [`Stream`]
//! where the `Item` implements [`Eventable`]. Use it when you already have a conn in hand and
//! want to respond with an event stream.
//!
//! ```
//! use broadcaster::BroadcastChannel;
//! use trillium::{Conn, conn_unwrap};
//! use trillium_sse::SseConnExt;
//!
//! type Channel = BroadcastChannel<String>;
//!
//! fn get_sse(mut conn: Conn) -> Conn {
//!     let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
//!     conn.with_sse_stream(broadcaster)
//! }
//! ```
//!
//! Often, you will want this stream to be something like a channel, but
//! the specifics of that are dependent on the event fanout
//! characteristics of your application.
//!
//! ## Events
//!
//! This crate implements [`Eventable`] for an [`Event`] type that you can
//! use in your application, for `String`, and for `&'static str`. You can
//! also implement [`Eventable`] for any type in your application.
//!
//! In addition to data events, the stream can carry comments — messages that clients ignore,
//! sent periodically so that an idle stream is still producing traffic. See
//! [`Event::new_comment`], or
//! [`Sse::with_heartbeat`] /
//! [`with_sse_stream_and_heartbeat`](crate::SseConnExt::with_sse_stream_and_heartbeat) to have them
//! sent automatically whenever the stream goes quiet.
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

mod handler;
mod heartbeat;

use futures_lite::{AsyncRead, stream::Stream};
pub use handler::{Sse, SseHandler, sse};
use heartbeat::WithHeartbeat;
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
use trillium_server_common::Runtime;

struct SseBody<S, E> {
    stream: S,
    buffer: Vec<u8>,
    event: PhantomData<E>,
}

impl<S, E> SseBody<S, E>
where
    S: Stream<Item = E> + Unpin + Send + 'static,
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
        writeln!(&mut output, "event: {event_type}").ok()?;
    }

    if let Some(id) = event.id() {
        writeln!(&mut output, "id: {id}").ok()?;
    }

    if let Some(retry) = event.retry() {
        writeln!(&mut output, "retry: {}", retry.as_millis()).ok()?;
    }

    if let Some(data) = event.data() {
        write_multiline_field(&mut output, "data:", data);
    }

    if output.is_empty() {
        None
    } else {
        writeln!(&mut output).ok()?;
        Some(output)
    }
}

impl<S, E> AsyncRead for SseBody<S, E>
where
    S: Stream<Item = E> + Unpin + Send + 'static,
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
    S: Stream<Item = E> + Unpin + Send + 'static,
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
        S: Stream<Item = E> + Unpin + Send + 'static,
        E: Eventable;

    /// as [`with_sse_stream`](SseConnExt::with_sse_stream), but sends an empty comment whenever
    /// `heartbeat` elapses without the stream yielding an event.
    ///
    /// The interval is measured from the most recent event, not from the start of the response,
    /// so a busy stream sends no heartbeats at all. Clients discard the comment.
    fn with_sse_stream_and_heartbeat<S, E>(self, sse_stream: S, heartbeat: Duration) -> Self
    where
        S: Stream<Item = E> + Unpin + Send + 'static,
        E: Eventable;
}

impl SseConnExt for Conn {
    fn with_sse_stream<S, E>(self, sse_stream: S) -> Self
    where
        S: Stream<Item = E> + Unpin + Send + 'static,
        E: Eventable,
    {
        let body = SseBody::new(self.swansong().interrupt(sse_stream));
        self.set_sse_headers().with_body(body)
    }

    fn with_sse_stream_and_heartbeat<S, E>(self, sse_stream: S, heartbeat: Duration) -> Self
    where
        S: Stream<Item = E> + Unpin + Send + 'static,
        E: Eventable,
    {
        let Some(runtime) = self.shared_state::<Runtime>().cloned() else {
            log::warn!(
                "no runtime in shared state; sending sse stream without a heartbeat. this conn \
                 was probably not served by a trillium server."
            );
            return self.with_sse_stream(sse_stream);
        };

        let stream = WithHeartbeat::new(sse_stream, runtime, heartbeat);
        let body = SseBody::new(self.swansong().interrupt(stream));
        self.set_sse_headers().with_body(body)
    }
}

trait SseHeaders {
    fn set_sse_headers(self) -> Self;
}

impl SseHeaders for Conn {
    fn set_sse_headers(self) -> Self {
        self.with_response_header(KnownHeaderName::ContentType, "text/event-stream")
            .with_response_header(KnownHeaderName::CacheControl, "no-cache")
            // Close-delimited framing: the event stream carries neither `Content-Length`
            // nor `Transfer-Encoding`, running until the connection closes. Chunked
            // transfer-encoding can disrupt event delivery timing for this protocol.
            .with_response_header(KnownHeaderName::Connection, "close")
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
    /// Comments are ignored by clients. They are chiefly used as a heartbeat, sent periodically
    /// so that an idle event stream is still producing traffic.
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
#[derive(Debug, Clone, Eq, PartialEq, Default, fieldwork::Fieldwork)]
#[fieldwork(get, set, get_mut, with, option_set_some, into)]
pub struct Event {
    /// the data for this event
    data: Option<Cow<'static, str>>,
    /// a comment for this event
    comment: Option<Cow<'static, str>>,
    /// the type for this event
    #[field(with = with_type, set = set_type, get_mut = type_mut)]
    event_type: Option<Cow<'static, str>>,
    /// the id for this event
    id: Option<Cow<'static, str>>,
    /// reconnection time for this stream
    #[field(copy, into = false)]
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
    /// let event = trillium_sse::Event::new_comment("heartbeat");
    /// assert_eq!(event.comment(), Some("heartbeat"));
    /// assert_eq!(event.data(), None);
    /// ```
    pub fn new_comment(comment: impl Into<Cow<'static, str>>) -> Self {
        Self {
            comment: Some(comment.into()),
            ..Self::default()
        }
    }
}
