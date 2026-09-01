//! # Trillium tools for server sent events
//!
//! [`sse`] builds a [`Handler`](trillium::Handler) from any [`SseHandler`], which produces a
//! [`Stream`](futures_lite::Stream) of [`Eventable`] items per connected client. Each event is
//! written to the client as the stream yields it.
//!
//! Requests whose `Accept` header excludes `text/event-stream` are passed through to subsequent
//! handlers untouched.
//!
//! When a client goes away, the event stream is dropped promptly on every protocol, so a
//! stream backed by a subscription can use `Drop` to unsubscribe. A client that vanishes
//! without signalling — a killed process, a severed network — is noticed once the transport
//! notices, which on HTTP/3 is QUIC's negotiated idle timeout.
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
//! [`Event::new_comment`], or [`Sse::with_heartbeat`] to have them sent automatically whenever
//! the stream goes quiet.
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

pub use handler::{Sse, SseHandler, sse};
use std::{borrow::Cow, fmt::Write, time::Duration};

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
pub(crate) fn encode(event: &impl Eventable) -> Option<String> {
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
