use crate::{Eventable, SseConnExt, heartbeat::WithHeartbeat};
use futures_lite::Stream;
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    time::Duration,
};
use trillium::{Conn, Handler, Info, KnownHeaderName};
use trillium_server_common::Runtime;

/// The trait that defines an event source for the [`Sse`] handler.
///
/// Implement this on a type that holds whatever fanout mechanism your application uses — a
/// broadcast channel, a subscription registry — and return a per-client [`Stream`] from
/// [`connect`](SseHandler::connect).
///
/// ```
/// use broadcaster::BroadcastChannel;
/// use trillium::Conn;
/// use trillium_sse::{Sse, SseHandler};
///
/// struct Notifications {
///     channel: BroadcastChannel<String>,
/// }
///
/// impl SseHandler for Notifications {
///     type Event = String;
///     type EventStream = BroadcastChannel<String>;
///
///     async fn connect(&self, _conn: &mut Conn) -> Self::EventStream {
///         self.channel.clone()
///     }
/// }
///
/// let handler = Sse::new(Notifications {
///     channel: BroadcastChannel::new(),
/// });
/// ```
///
/// This trait is also implemented for any `Fn(&mut Conn) -> Stream`, for the common case
/// where nothing needs to be awaited in order to build the stream:
///
/// ```
/// use futures_lite::stream;
/// use trillium::Conn;
/// use trillium_sse::sse;
///
/// let handler = sse(|_: &mut Conn| stream::iter(["one", "two"]));
/// ```
pub trait SseHandler: Send + Sync + Sized + 'static {
    /// The type yielded by this handler's [`EventStream`](SseHandler::EventStream).
    type Event: Eventable;

    /// A [`Stream`] of events to send to a connected client, built per client in
    /// [`connect`](SseHandler::connect).
    type EventStream: Stream<Item = Self::Event> + Unpin + Send + 'static;

    /// Called once per request, to build the stream of events for that client.
    ///
    /// Returning `None` leaves the conn untouched, so it continues on to subsequent handlers.
    /// The conn is borrowed mutably to allow setting response headers or state, but note that
    /// [`Sse`] sets the status, headers, and body itself when a stream is returned.
    fn connect(&self, conn: &mut Conn) -> impl Future<Output = Self::EventStream> + Send;
}

impl<F, S, E> SseHandler for F
where
    F: Fn(&mut Conn) -> S + Send + Sync + 'static,
    S: Stream<Item = E> + Unpin + Send + 'static,
    E: Eventable,
{
    type Event = E;
    type EventStream = S;

    async fn connect(&self, conn: &mut Conn) -> Self::EventStream {
        self(conn)
    }
}

/// A [`Handler`] that responds to requests with a server-sent event stream.
///
/// Build one from any [`SseHandler`] with [`Sse::new`] or [`sse`]. Unlike
/// [`SseConnExt::with_sse_stream`], this can send heartbeat comments, because it obtains a
/// runtime from the server at startup.
///
/// The conn is passed through untouched — continuing on to subsequent handlers — if the request's
/// `Accept` header excludes `text/event-stream`, or if [`SseHandler::connect`] returns `None`. A
/// request with no `Accept` header accepts anything, per [RFC 9110 §12.5.1][rfc].
///
/// [rfc]: https://www.rfc-editor.org/rfc/rfc9110.html#name-accept
pub struct Sse<H> {
    handler: H,
    heartbeat: Option<Duration>,
    runtime: Option<Runtime>,
}

/// Builds a new [`Sse`] handler. Alias for [`Sse::new`].
pub fn sse<H: SseHandler>(sse_handler: H) -> Sse<H> {
    Sse::new(sse_handler)
}

impl<H: SseHandler> Sse<H> {
    /// Builds a new [`Sse`] handler from any [`SseHandler`].
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            heartbeat: None,
            runtime: None,
        }
    }

    /// Sends an empty comment whenever `heartbeat` elapses without the stream yielding an event.
    ///
    /// The interval is measured from the most recent event, not from the start of the response,
    /// so a busy stream sends no heartbeats at all. Clients discard the comment.
    ///
    /// This is a contract about what the server sends, not a guarantee about the connection.
    /// Regular traffic makes an idle stream less likely to be dropped by an intermediary, and
    /// makes the server notice a departed client sooner — it only learns of one when it next
    /// writes — but neither is assured.
    pub fn with_heartbeat(mut self, heartbeat: Duration) -> Self {
        self.heartbeat = Some(heartbeat);
        self
    }
}

/// Whether the client is willing to receive `text/event-stream`.
///
/// A request with no `Accept` header accepts anything. An otherwise-matching media range with
/// `q=0` is a refusal. Preference ordering is not considered, as there is only one media type on
/// offer — the question is acceptability, not which of several to send.
fn accepts_event_stream(conn: &Conn) -> bool {
    let Some(accept) = conn.request_headers().get_str(KnownHeaderName::Accept) else {
        return true;
    };

    accept.split(',').any(|media_range| {
        let mut parts = media_range.split(';').map(str::trim);

        let matches_range = parts.next().is_some_and(|range| {
            ["text/event-stream", "text/*", "*/*"]
                .iter()
                .any(|acceptable| range.eq_ignore_ascii_case(acceptable))
        });

        matches_range
            && !parts.any(|parameter| {
                parameter
                    .strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
                    .is_some_and(|q| q <= 0.0)
            })
    })
}

impl<H: SseHandler> Handler for Sse<H> {
    async fn run(&self, mut conn: Conn) -> Conn {
        if !accepts_event_stream(&conn) {
            return conn;
        }

        let stream = self.handler.connect(&mut conn).await;

        match (self.heartbeat, &self.runtime) {
            (Some(heartbeat), Some(runtime)) => {
                conn.with_sse_stream(WithHeartbeat::new(stream, runtime.clone(), heartbeat))
            }
            _ => conn.with_sse_stream(stream),
        }
    }

    async fn init(&mut self, info: &mut Info) {
        self.runtime = info.shared_state::<Runtime>().cloned();

        if self.heartbeat.is_some() && self.runtime.is_none() {
            log::warn!(
                "no runtime in shared state; sse heartbeats are disabled. this handler was \
                 probably not initialized by a trillium runtime adapter."
            );
        }
    }
}

impl<H> Debug for Sse<H>
where
    H: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sse")
            .field("handler", &self.handler)
            .field("heartbeat", &self.heartbeat)
            .field("runtime", &self.runtime)
            .finish()
    }
}
