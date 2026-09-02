use crate::{Eventable, encode};
use futures_lite::{AsyncWriteExt, Stream};
use std::{
    fmt::{self, Debug, Formatter},
    future::{self, Future},
    io,
    pin::Pin,
    task::Poll,
    time::Duration,
};
use sync_wrapper::SyncWrapper;
use trillium::{Conn, Handler, Info, KnownHeaderName, Status, Upgrade};
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
    /// The conn is borrowed mutably to allow setting response headers or state, but note that
    /// [`Sse`] sets the status, headers, and body itself.
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
/// Build one from any [`SseHandler`] with [`Sse::new`] or [`sse`].
///
/// The conn is passed through untouched — continuing on to subsequent handlers — unless the
/// request's [`Accept`][rfc] header names `text/event-stream`. Wildcard ranges and an absent
/// `Accept` header do not match, so the same route can also serve other representations.
///
/// When the client disconnects, the event stream is dropped: promptly on HTTP/1.x and HTTP/2,
/// and at the next event or heartbeat on HTTP/3.
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
    /// Regular traffic makes an idle stream less likely to be dropped by an intermediary. On
    /// HTTP/3 it also bounds how long a departed client goes unnoticed, since disconnection is
    /// detected there by a failed write.
    pub fn with_heartbeat(mut self, heartbeat: Duration) -> Self {
        self.heartbeat = Some(heartbeat);
        self
    }
}

/// Whether the client asked for `text/event-stream` by name.
///
/// Wildcards (`*/*`, `text/*`) and an absent `Accept` header do not count: they say the client
/// will take whatever is on offer, not that it speaks the event-stream protocol. Requiring the
/// media type by name lets an `Sse` handler share a route with handlers serving other
/// representations. An `Accept` header naming it with `q=0` is a refusal.
fn accepts_event_stream(conn: &Conn) -> bool {
    let Some(accept) = conn.request_headers().get_str(KnownHeaderName::Accept) else {
        return false;
    };

    accept.split(',').any(|media_range| {
        let mut parts = media_range.split(';').map(str::trim);

        let matches_range = parts
            .next()
            .is_some_and(|range| range.eq_ignore_ascii_case("text/event-stream"));

        matches_range
            && !parts.any(|parameter| {
                parameter
                    .strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
                    .is_some_and(|q| q <= 0.0)
            })
    })
}

/// Private state key carrying the event stream from [`Handler::run`] to [`Handler::upgrade`].
/// The [`SyncWrapper`] lets a `!Sync` stream satisfy the state
/// [`TypeSet`](trillium::TypeSet)'s `Sync` requirement.
struct SseStream<S>(SyncWrapper<S>);

/// Stray inbound bytes tolerated while probing for disconnection on h1. A conforming client
/// sends nothing on an event stream, but a pipelining client may have optimistic requests in
/// flight.
const READ_ALLOWANCE: usize = 16 * 1024;

enum Tick<E> {
    Event(E),
    Heartbeat,
    StreamEnded,
    ClientDisconnected,
}

async fn write_flush(upgrade: &mut Upgrade, bytes: &[u8]) -> io::Result<()> {
    upgrade.write_all(bytes).await?;
    upgrade.flush().await
}

async fn drive_events<S, E>(mut upgrade: Upgrade, stream: S, heartbeat: Option<(Duration, Runtime)>)
where
    S: Stream<Item = E> + Unpin + Send + 'static,
    E: Eventable,
{
    let swansong = upgrade.swansong();
    let mut stream = swansong.interrupt(stream);

    let new_delay = |(duration, runtime): &(Duration, Runtime)| {
        let duration = *duration;
        let runtime = runtime.clone();
        Box::pin(async move { runtime.delay(duration).await })
            as Pin<Box<dyn Future<Output = ()> + Send>>
    };
    let mut delay = heartbeat.as_ref().map(new_delay);

    loop {
        let tick = future::poll_fn(|cx| {
            if Pin::new(upgrade.as_mut())
                .poll_closed(cx, READ_ALLOWANCE)
                .is_ready()
            {
                return Poll::Ready(Tick::ClientDisconnected);
            }

            match Pin::new(&mut stream).poll_next(cx) {
                Poll::Ready(Some(event)) => return Poll::Ready(Tick::Event(event)),
                Poll::Ready(None) => return Poll::Ready(Tick::StreamEnded),
                Poll::Pending => {}
            }

            if let Some(delay) = &mut delay
                && delay.as_mut().poll(cx).is_ready()
            {
                return Poll::Ready(Tick::Heartbeat);
            }

            Poll::Pending
        })
        .await;

        match tick {
            Tick::ClientDisconnected => return,
            Tick::StreamEnded => break,
            Tick::Event(event) => {
                delay = heartbeat.as_ref().map(new_delay);
                let Some(encoded) = encode(&event) else {
                    continue;
                };
                if write_flush(&mut upgrade, encoded.as_bytes()).await.is_err() {
                    return;
                }
            }
            Tick::Heartbeat => {
                delay = heartbeat.as_ref().map(new_delay);
                if write_flush(&mut upgrade, b":\n\n").await.is_err() {
                    return;
                }
            }
        }
    }

    let _ = upgrade.close().await;
}

impl<H: SseHandler> Handler for Sse<H> {
    async fn run(&self, mut conn: Conn) -> Conn {
        if !accepts_event_stream(&conn) {
            return conn;
        }

        let stream = self.handler.connect(&mut conn).await;

        conn.with_state(SseStream(SyncWrapper::new(stream)))
            .with_response_header(KnownHeaderName::ContentType, "text/event-stream")
            .with_response_header(KnownHeaderName::CacheControl, "no-cache")
            // Close-delimited framing: the event stream carries neither `Content-Length`
            // nor `Transfer-Encoding`, running until the connection closes. Chunked
            // transfer-encoding can disrupt event delivery timing for this protocol.
            // h2 and h3 strip this h1-only header and frame at the stream layer.
            .with_response_header(KnownHeaderName::Connection, "close")
            .with_status(Status::Ok)
            .halt()
            .upgrade()
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

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state().contains::<SseStream<H::EventStream>>()
    }

    async fn upgrade(&self, mut upgrade: Upgrade) {
        let Some(SseStream(stream)) = upgrade.state_mut().take::<SseStream<H::EventStream>>()
        else {
            return;
        };
        let stream = stream.into_inner();

        let heartbeat = self.heartbeat.zip(self.runtime.clone());
        drive_events(upgrade, stream, heartbeat).await;
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
