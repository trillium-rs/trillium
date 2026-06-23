//! Client-side [Server-Sent Events][spec].
//!
//! [`Conn::into_sse`] executes a request and interprets the response body as an
//! `text/event-stream`, yielding a [`Stream`] of [`Event`]s. Unlike
//! [`into_websocket`][Conn::into_websocket], this is not a protocol upgrade — an event stream is
//! an ordinary response whose body is read incrementally and parsed line-by-line. It works
//! identically over HTTP/1.x, HTTP/2, and HTTP/3.
//!
//! This is a single-response stream: it ends when the connection closes. It does **not**
//! implement the [`EventSource`][es] automatic-reconnection behavior (re-issuing the request with
//! `Last-Event-ID` and honoring server `retry:` timing), which only makes sense for idempotent
//! event feeds. To retry a dropped request, drive the whole request through a retrying
//! [`ClientHandler`][crate::ClientHandler].
//!
//! [spec]: https://html.spec.whatwg.org/multipage/server-sent-events.html
//! [es]: https://developer.mozilla.org/en-US/docs/Web/API/EventSource

use crate::{Conn, ResponseBody};
use futures_lite::{AsyncRead, stream::Stream};
use std::{
    collections::VecDeque,
    error::Error,
    fmt::{self, Debug, Display, Formatter},
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};
use trillium_http::{KnownHeaderName, Status};

const READ_BUF_LEN: usize = 8 * 1024;

impl Conn {
    /// Execute this request and interpret the response body as a [Server-Sent Events][spec]
    /// stream.
    ///
    /// This is an *execution* method: it sends the request (setting `Accept: text/event-stream`
    /// if not already present), then validates that the response has a success status and a
    /// `text/event-stream` content-type before handing back an [`EventStream`]. Calling it on a
    /// conn that has already been awaited returns [`SseErrorKind::AlreadyExecuted`] — build the
    /// conn, then call this; don't await it yourself first.
    ///
    /// On any failure the returned [`SseError`] still carries the [`Conn`], so the caller can
    /// inspect the response (status, headers, error body) or convert it back with
    /// [`From`]/[`Into`].
    ///
    /// [spec]: https://html.spec.whatwg.org/multipage/server-sent-events.html
    pub async fn into_sse(mut self) -> Result<EventStream, SseError> {
        if self.status().is_some() {
            return Err(SseError::new(self, SseErrorKind::AlreadyExecuted));
        }

        self.request_headers_mut()
            .try_insert(KnownHeaderName::Accept, "text/event-stream");

        if let Err(e) = (&mut self).await {
            return Err(SseError::new(self, e.into()));
        }

        let status = self.status().expect("Response did not include status");
        if !status.is_success() {
            return Err(SseError::new(self, SseErrorKind::Status(status)));
        }

        if !is_event_stream(
            self.response_headers()
                .get_str(KnownHeaderName::ContentType),
        ) {
            let content_type = self
                .response_headers()
                .get_str(KnownHeaderName::ContentType)
                .map(String::from);
            return Err(SseError::new(
                self,
                SseErrorKind::UnexpectedContentType(content_type),
            ));
        }

        match self.take_response_body() {
            Some(body) => Ok(EventStream::new(body)),
            None => Err(SseError::new(self, SseErrorKind::NoBody)),
        }
    }
}

/// True if `content_type` names the `text/event-stream` media type, ignoring any parameters
/// (e.g. `; charset=utf-8`) and ASCII case.
fn is_event_stream(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|ct| {
        ct.split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))
    })
}

/// A single server-sent event.
///
/// Field accessors follow the [SSE specification][spec]: [`event_type`](Event::event_type) is
/// `None` for the default `message` type, [`data`](Event::data) has had its lines joined with
/// `\n` and the trailing newline removed, and [`id`](Event::id) reflects the most recent `id:`
/// field seen on the stream (it persists across events, matching `EventSource.lastEventId`).
///
/// [spec]: https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Event {
    data: String,
    event_type: Option<String>,
    id: Option<String>,
    retry: Option<Duration>,
}

impl Event {
    /// The event payload, with multiple `data:` lines joined by `\n`.
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }

    /// The event type from the `event:` field, or `None` for the default `message` type.
    #[must_use]
    pub fn event_type(&self) -> Option<&str> {
        self.event_type.as_deref()
    }

    /// The last event id seen on the stream up to and including this event.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The server-requested reconnection time from a `retry:` field, if one preceded this event.
    ///
    /// This is a connection-level directive; because [`EventStream`] does not reconnect, it is
    /// surfaced purely informationally for callers that implement their own reconnection.
    #[must_use]
    pub fn retry(&self) -> Option<Duration> {
        self.retry
    }
}

/// A [`Stream`] of [`Event`]s decoded from a `text/event-stream` response body.
///
/// Created by [`Conn::into_sse`]. The stream yields `Result<Event, trillium_http::Error>`; an
/// error item is an IO failure reading the underlying transport, after which the stream ends.
/// The stream ends with `None` when the connection closes; an incomplete event at end-of-stream
/// (no terminating blank line) is discarded per the specification.
#[derive(Debug)]
pub struct EventStream {
    body: ResponseBody<'static>,
    decoder: Decoder,
    pending: VecDeque<Event>,
    read_buf: Box<[u8]>,
    done: bool,
}

impl EventStream {
    fn new(body: ResponseBody<'static>) -> Self {
        Self {
            body,
            decoder: Decoder::default(),
            pending: VecDeque::new(),
            read_buf: vec![0; READ_BUF_LEN].into_boxed_slice(),
            done: false,
        }
    }
}

impl Stream for EventStream {
    type Item = trillium_http::Result<Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(event) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if this.done {
                return Poll::Ready(None);
            }
            match ready!(Pin::new(&mut this.body).poll_read(cx, &mut this.read_buf)) {
                // EOF: a trailing event without its blank line is discarded per spec.
                Ok(0) => {
                    this.done = true;
                    return Poll::Ready(None);
                }
                Ok(n) => this.decoder.push(&this.read_buf[..n], &mut this.pending),
                Err(e) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(e.into())));
                }
            }
        }
    }
}

/// Incremental, allocation-reusing parser for the SSE wire format.
///
/// Bytes are fed in arbitrary chunks via [`push`](Decoder::push); completed [`Event`]s are
/// appended to the caller's queue. Line terminators (CR, LF, CRLF) are handled across chunk
/// boundaries via `last_char_was_cr`.
#[derive(Debug, Default)]
struct Decoder {
    line: Vec<u8>,
    last_char_was_cr: bool,
    data: String,
    event_type: Option<String>,
    id: Option<String>,
    retry: Option<Duration>,
    has_data: bool,
}

impl Decoder {
    fn push(&mut self, bytes: &[u8], out: &mut VecDeque<Event>) {
        for &byte in bytes {
            match byte {
                b'\r' => {
                    self.line_done(out);
                    self.last_char_was_cr = true;
                }
                b'\n' if self.last_char_was_cr => self.last_char_was_cr = false,
                b'\n' => self.line_done(out),
                _ => {
                    self.last_char_was_cr = false;
                    self.line.push(byte);
                }
            }
        }
    }

    fn line_done(&mut self, out: &mut VecDeque<Event>) {
        if self.line.is_empty() {
            self.dispatch(out);
        } else {
            let mut line = std::mem::take(&mut self.line);
            self.process_field(&line);
            line.clear();
            self.line = line;
        }
    }

    fn process_field(&mut self, line: &[u8]) {
        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(0) => return, // leading colon: comment
            Some(colon) => {
                let value = &line[colon + 1..];
                let value = value.strip_prefix(b" ").unwrap_or(value);
                (&line[..colon], value)
            }
            None => (line, &b""[..]),
        };

        match field {
            b"event" => self.event_type = Some(String::from_utf8_lossy(value).into_owned()),
            b"data" => {
                self.data.push_str(&String::from_utf8_lossy(value));
                self.data.push('\n');
                self.has_data = true;
            }
            b"id" => {
                if !value.contains(&0) {
                    self.id = Some(String::from_utf8_lossy(value).into_owned());
                }
            }
            b"retry" => {
                if !value.is_empty()
                    && value.iter().all(u8::is_ascii_digit)
                    && let Ok(ms) = std::str::from_utf8(value).unwrap_or_default().parse()
                {
                    self.retry = Some(Duration::from_millis(ms));
                }
            }
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut VecDeque<Event>) {
        if !self.has_data {
            // No data accumulated: reset the data and event-type buffers without dispatching,
            // but leave `id` (last-event-id) and any pending `retry` intact, per spec.
            self.data.clear();
            self.event_type = None;
            return;
        }

        if self.data.ends_with('\n') {
            self.data.pop();
        }

        out.push_back(Event {
            data: std::mem::take(&mut self.data),
            event_type: self.event_type.take().filter(|s| !s.is_empty()),
            id: self.id.clone(),
            retry: self.retry.take(),
        });
        self.has_data = false;
    }
}

/// The kind of error that occurred attempting to open an [`EventStream`].
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum SseErrorKind {
    /// An HTTP error attempting to make the request.
    #[error(transparent)]
    Http(#[from] trillium_http::Error),

    /// The response status was not a success (2xx).
    #[error("Unexpected response status {0} for SSE request")]
    Status(Status),

    /// The response content-type was not `text/event-stream`.
    #[error("Unexpected content-type for SSE request: {0:?}")]
    UnexpectedContentType(Option<String>),

    /// [`Conn::into_sse`] was called on a [`Conn`] that had already been executed (its status is
    /// already set). The request *is* the execution; build the conn and await `into_sse`
    /// directly without awaiting first.
    #[error(
        "Conn::into_sse called after execution — build the conn and await into_sse instead of \
         awaiting the conn separately"
    )]
    AlreadyExecuted,

    /// The response had no body to read as an event stream.
    #[error("SSE response had no body")]
    NoBody,
}

/// An attempt to open an [`EventStream`] via [`Conn::into_sse`] failed.
///
/// This dereferences to the [`Conn`] and converts back into it with [`From`]/[`Into`], so the
/// caller can inspect the response that caused the failure.
#[derive(Debug)]
pub struct SseError {
    /// The kind of error that occurred.
    pub kind: SseErrorKind,
    conn: Box<Conn>,
}

impl SseError {
    fn new(conn: Conn, kind: SseErrorKind) -> Self {
        Self {
            kind,
            conn: Box::new(conn),
        }
    }
}

impl From<SseError> for Conn {
    fn from(value: SseError) -> Self {
        *value.conn
    }
}

impl Deref for SseError {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl DerefMut for SseError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

impl Error for SseError {}

impl Display for SseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.kind, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `input` to a fresh decoder in one chunk, then again one byte at a time, asserting
    /// both produce the same events. Splitting per byte exercises the cross-chunk line-terminator
    /// and field-accumulation state.
    fn decode(input: &[u8]) -> Vec<Event> {
        let mut whole = Decoder::default();
        let mut whole_out = VecDeque::new();
        whole.push(input, &mut whole_out);

        let mut split = Decoder::default();
        let mut split_out = VecDeque::new();
        for byte in input {
            split.push(&[*byte], &mut split_out);
        }

        assert_eq!(whole_out, split_out, "chunked decode diverged from whole");
        whole_out.into()
    }

    #[test]
    fn fields_comments_and_terminators() {
        let events =
            decode(b": this is a comment\nevent: greeting\ndata: hello\nid: 42\nretry: 3000\n\n");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.data(), "hello");
        assert_eq!(event.event_type(), Some("greeting"));
        assert_eq!(event.id(), Some("42"));
        assert_eq!(event.retry(), Some(Duration::from_millis(3000)));
    }

    #[test]
    fn multiline_data_joins_with_newline() {
        let events = decode(b"data: one\ndata: two\ndata:three\n\n");
        // Only the single space after the first colon is stripped; "data:three" has none.
        assert_eq!(events[0].data(), "one\ntwo\nthree");
    }

    #[test]
    fn crlf_and_cr_terminators() {
        let crlf = decode(b"data: a\r\n\r\n");
        assert_eq!(crlf[0].data(), "a");
        let cr = decode(b"data: b\r\r");
        assert_eq!(cr[0].data(), "b");
    }

    #[test]
    fn empty_data_line_dispatches_empty_event() {
        // A bare `data` field (no value) still counts as data and dispatches.
        let events = decode(b"data\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data(), "");
    }

    #[test]
    fn blank_lines_without_data_dispatch_nothing() {
        assert!(decode(b"\n\n\n").is_empty());
        assert!(decode(b": just a comment\n\n").is_empty());
    }

    #[test]
    fn incomplete_trailing_event_is_discarded() {
        // No terminating blank line: the event is never dispatched.
        assert!(decode(b"data: pending\n").is_empty());
    }

    #[test]
    fn id_persists_across_events_retry_does_not() {
        let events = decode(b"id: 1\nretry: 500\ndata: a\n\ndata: b\n\n");
        assert_eq!(events[0].id(), Some("1"));
        assert_eq!(events[0].retry(), Some(Duration::from_millis(500)));
        // `id` is the last-event-id and carries forward; `retry` is consumed by the first event.
        assert_eq!(events[1].id(), Some("1"));
        assert_eq!(events[1].retry(), None);
    }

    #[test]
    fn invalid_retry_is_ignored() {
        let events = decode(b"retry: not-a-number\ndata: a\n\n");
        assert_eq!(events[0].retry(), None);
    }
}
