/*!
# Trillium tools for server sent events

This primarily provides [`SseConnExt`](crate::SseConnExt), an
extension trait for [`trillium::Conn`] that has a
[`with_sse_stream`](crate::SseConnExt::with_sse_stream) chainable
method that takes a [`Stream`](futures_lite::Stream) where the `Item`
implements [`Eventable`].

Often, you will want this stream to be something like a channel, but
the specifics of that are dependent on the event fanout
characteristics of your application.

This crate implements [`Eventable`] for an [`Event`] type that you can
use in your application, for `String`, and for `&'static str`. You can
also implement [`Eventable`] for any type in your application.

## Example usage

```
use broadcaster::BroadcastChannel;
use trillium::{conn_try, conn_unwrap, log_error, Conn, Method, State};
use trillium_sse::SseConnExt;
use trillium_static_compiled::static_compiled;

type Channel = BroadcastChannel<String>;

fn get_sse(mut conn: Conn) -> Conn {
    let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
    conn.with_sse_stream(broadcaster)
}

async fn post_broadcast(mut conn: Conn) -> Conn {
    let broadcaster = conn_unwrap!(conn.take_state::<Channel>(), conn);
    let body = conn_try!(conn.request_body_string().await, conn);
    log_error!(broadcaster.send(&body).await);
    conn.ok("sent")
}

fn main() {
    let handler = (
        static_compiled!("examples/static").with_index_file("index.html"),
        State::new(Channel::new()),
        |conn: Conn| async move {
            match (conn.method(), conn.path()) {
                (Method::Get, "/sse") => get_sse(conn),
                (Method::Post, "/broadcast") => post_broadcast(conn).await,
                _ => conn,
            }
        },
    );

    // trillium_smol::run(handler);
}

```
*/
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs)]

use futures_lite::{stream::Stream, AsyncRead};
use std::{
    borrow::Cow,
    fmt::Write,
    io,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
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
            event: PhantomData::default(),
        }
    }
}

fn encode(event: impl Eventable) -> String {
    let mut output = String::new();
    if let Some(event_type) = event.event_type() {
        writeln!(&mut output, "event: {}", event_type).unwrap();
    }

    if let Some(id) = event.id() {
        writeln!(&mut output, "id: {}", id).unwrap();
    }

    for part in event.data().lines() {
        writeln!(&mut output, "data: {}", part).unwrap();
    }

    writeln!(output).unwrap();

    output
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

        match Pin::new(stream).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(item)) => {
                let data = encode(item).into_bytes();
                let writable_len = data.len().min(buf.len());
                buf[0..writable_len].copy_from_slice(&data[0..writable_len]);
                if writable_len < data.len() {
                    buffer.extend_from_slice(&data[writable_len..]);
                }
                Poll::Ready(Ok(writable_len))
            }

            Poll::Ready(None) => Poll::Ready(Ok(0)),
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

/**
Extension trait for server sent events
*/
pub trait SseConnExt {
    /**
    builds and sets a streaming response body that conforms to the
    [server-sent-events
    spec](https://html.spec.whatwg.org/multipage/server-sent-events.html#server-sent-events)
    from a Stream of any [`Eventable`](crate::Eventable) type (such as
    [`Event`](crate::Event), as well as setting appropiate headers for
    this response.
    */
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
        let body = SseBody::new(self.inner().stopper().stop_stream(sse_stream));
        self.with_header(KnownHeaderName::ContentType, "text/event-stream")
            .with_header(KnownHeaderName::CacheControl, "no-cache")
            .with_body(body)
            .with_status(Status::Ok)
            .halt()
    }
}

/**
A trait that allows any Unpin + Send + Sync type to act as an event.

For a concrete implementation of this trait, you can use [`Event`],
but it is also implemented for [`String`] and [`&'static str`].
*/

pub trait Eventable: Unpin + Send + Sync + 'static {
    /// return the data for this event. non-optional.
    fn data(&self) -> &str;

    /// return the event type, optionally
    fn event_type(&self) -> Option<&str> {
        None
    }

    /// return a unique event id, optionally
    fn id(&self) -> Option<&str> {
        None
    }
}

impl Eventable for Event {
    fn data(&self) -> &str {
        Event::data(self)
    }

    fn event_type(&self) -> Option<&str> {
        Event::event_type(self)
    }
}

impl Eventable for &'static str {
    fn data(&self) -> &str {
        self
    }
}

impl Eventable for String {
    fn data(&self) -> &str {
        self
    }
}

/**
Events are a concrete implementation of the [`Eventable`] trait.
*/
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Event {
    data: Cow<'static, str>,
    event_type: Option<Cow<'static, str>>,
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
            data,
            event_type: None,
        }
    }
}

impl Event {
    /**
    builds a new [`Event`]

    by default, this event has no event type. to set an event type,
    use [`Event::with_type`] or [`Event::set_type`]
    */
    pub fn new(data: impl Into<Cow<'static, str>>) -> Self {
        Self::from(data.into())
    }

    /**
    chainable constructor to set the type on an event

    ```
    let event = trillium_sse::Event::new("event data").with_type("userdata");
    assert_eq!(event.event_type(), Some("userdata"));
    assert_eq!(event.data(), "event data");
    ```
    */
    pub fn with_type(mut self, event_type: impl Into<Cow<'static, str>>) -> Self {
        self.set_type(event_type);
        self
    }

    /**
    set the event type for this Event. The default is None.

    ```
    let mut event = trillium_sse::Event::new("event data");
    assert_eq!(event.event_type(), None);
    event.set_type("userdata");
    assert_eq!(event.event_type(), Some("userdata"));
    ```
     */
    pub fn set_type(&mut self, event_type: impl Into<Cow<'static, str>>) {
        self.event_type = Some(event_type.into());
    }

    /// returns this Event's data as a &str
    pub fn data(&self) -> &str {
        &self.data
    }

    /// returns this Event's type as a str, if set
    pub fn event_type(&self) -> Option<&str> {
        self.event_type.as_deref()
    }
}
