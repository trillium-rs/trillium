//! End-to-end tests for the [`ClientHandler`] middleware extension point.
//!
//! These tests use a `ServerConnector` that responds 500, so any test that ends with a 200 is
//! proving that a handler short-circuited the network call.

use futures_lite::AsyncRead;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use trillium_client::{Body, Client, ClientHandler, Conn, ConnExt, Status, Url};
use trillium_http::KnownHeaderName::ContentLength;
use trillium_server_common::Connector;
use trillium_testing::{ServerConnector, TestResult, harness, test};

#[derive(Debug, Default)]
struct Counter {
    runs: AtomicUsize,
    after_responses: AtomicUsize,
}

impl ClientHandler for Counter {
    async fn run(&self, _conn: &mut Conn) -> trillium_client::Result<()> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn after_response(&self, _conn: &mut Conn) -> trillium_client::Result<()> {
        self.after_responses.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug)]
struct Halter;

impl ClientHandler for Halter {
    async fn run(&self, conn: &mut Conn) -> trillium_client::Result<()> {
        conn.set_status(Status::Ok).set_response_body("synthesized");
        conn.response_headers_mut().insert(ContentLength, "11");
        conn.halt();
        Ok(())
    }
}

#[derive(Debug, Default)]
struct OrderRecorder {
    runs: std::sync::Mutex<Vec<&'static str>>,
    after_responses: std::sync::Mutex<Vec<&'static str>>,
}

#[derive(Debug)]
struct Tagged {
    tag: &'static str,
    recorder: std::sync::Arc<OrderRecorder>,
}

impl ClientHandler for Tagged {
    async fn run(&self, _conn: &mut Conn) -> trillium_client::Result<()> {
        self.recorder.runs.lock().unwrap().push(self.tag);
        Ok(())
    }

    async fn after_response(&self, _conn: &mut Conn) -> trillium_client::Result<()> {
        self.recorder.after_responses.lock().unwrap().push(self.tag);
        Ok(())
    }
}

#[test(harness)]
async fn single_handler_runs_both_passes() -> TestResult {
    let client = Client::new(ServerConnector::new(Status::Ok)).with_handler(Counter::default());

    let _conn = client.get("http://example.com/").await?;

    let counter = client
        .downcast_handler::<Counter>()
        .expect("handler installed");
    assert_eq!(counter.runs.load(Ordering::SeqCst), 1);
    assert_eq!(counter.after_responses.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test(harness)]
async fn handler_can_halt_and_synthesize_response() -> TestResult {
    // 500 from network, but Halter halts — so success means the chain short-circuited.
    let client =
        Client::new(ServerConnector::new(Status::InternalServerError)).with_handler(Halter);

    let mut conn = client.get("http://synthetic.invalid/").await?;
    assert_eq!(conn.status(), Some(Status::Ok));
    assert_eq!(conn.response_body().read_string().await?, "synthesized");
    Ok(())
}

// A streaming request-body reader that records when it is dropped. It never yields data — a
// halting handler must never read it — so a poll would park, standing in for any external
// producer the body is fed from.
struct DropSignalBody {
    dropped: Arc<AtomicBool>,
}

impl Drop for DropSignalBody {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl AsyncRead for DropSignalBody {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Pending
    }
}

#[test(harness)]
async fn halting_handler_drops_unsent_request_body() -> TestResult {
    // A handler that halts serves its own response, so the request body is never sent. The conn
    // must drop it rather than hold it — otherwise an external producer streaming into the body
    // (e.g. trillium-proxy pumping a forwarded request body) parks forever waiting to be read.
    let dropped = Arc::new(AtomicBool::new(false));
    let client =
        Client::new(ServerConnector::new(Status::InternalServerError)).with_handler(Halter);

    let mut conn = client
        .post("http://synthetic.invalid/")
        .with_body(Body::new_streaming(
            DropSignalBody {
                dropped: dropped.clone(),
            },
            None,
        ));
    (&mut conn).await?;

    assert_eq!(conn.status(), Some(Status::Ok));
    assert!(
        dropped.load(Ordering::SeqCst),
        "a halted conn must drop its unsent request body"
    );
    Ok(())
}

#[test(harness)]
async fn tuple_after_response_runs_in_reverse_after_halt() -> TestResult {
    // (Halter, Counter): Halter halts in run (skipping Counter::run), but after_response always
    // runs in reverse order, so Counter::after_response fires first, then Halter::after_response.
    let client = Client::new(ServerConnector::new(Status::InternalServerError))
        .with_handler((Halter, Counter::default()));

    let mut conn = client.get("http://synthetic.invalid/").await?;
    assert_eq!(conn.status(), Some(Status::Ok));
    assert_eq!(conn.response_body().read_string().await?, "synthesized");

    let (_halter, counter) = client
        .downcast_handler::<(Halter, Counter)>()
        .expect("handler installed");
    // Halter halts before Counter::run gets a chance.
    assert_eq!(counter.runs.load(Ordering::SeqCst), 0);
    // But after_response runs regardless of halt.
    assert_eq!(counter.after_responses.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test(harness)]
async fn tuple_runs_forward_and_after_responses_in_reverse() -> TestResult {
    let recorder = std::sync::Arc::new(OrderRecorder::default());
    let a = Tagged {
        tag: "A",
        recorder: recorder.clone(),
    };
    let b = Tagged {
        tag: "B",
        recorder: recorder.clone(),
    };
    let c = Tagged {
        tag: "C",
        recorder: recorder.clone(),
    };

    let client = Client::new(ServerConnector::new(Status::Ok)).with_handler((a, b, c));
    let _conn = client.get("http://example.com/").await?;

    assert_eq!(*recorder.runs.lock().unwrap(), vec!["A", "B", "C"]);
    assert_eq!(
        *recorder.after_responses.lock().unwrap(),
        vec!["C", "B", "A"]
    );
    Ok(())
}

#[test(harness)]
async fn unit_handler_is_default_and_no_op() -> TestResult {
    // A client without with_handler() defaults to (); awaiting still works.
    let client = Client::new(ServerConnector::new(Status::Ok));
    let conn = client.get("http://example.com/").await?;
    assert_eq!(conn.status(), Some(Status::Ok));
    Ok(())
}

#[test(harness)]
async fn downcast_handler_returns_none_for_wrong_type() -> TestResult {
    let client = Client::new(ServerConnector::new(Status::Ok)).with_handler(Counter::default());
    assert!(client.downcast_handler::<Halter>().is_none());
    assert!(client.downcast_handler::<Counter>().is_some());
    Ok(())
}

// Connector that always fails to connect — used to drive the
// transport-error code path in `Conn::exec`.
#[derive(Debug)]
struct FailingConnector {
    inner: ServerConnector<Status>,
}

impl FailingConnector {
    fn new() -> Self {
        Self {
            inner: ServerConnector::new(Status::Ok),
        }
    }
}

impl Connector for FailingConnector {
    type Runtime = <ServerConnector<Status> as Connector>::Runtime;
    type Transport = <ServerConnector<Status> as Connector>::Transport;
    type Udp = <ServerConnector<Status> as Connector>::Udp;

    async fn connect(&self, _url: &Url) -> io::Result<Self::Transport> {
        Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "test failure",
        ))
    }

    fn runtime(&self) -> Self::Runtime {
        self.inner.runtime().clone()
    }

    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        self.inner.resolve(host, port).await
    }
}

// Records what after_response saw, including whether conn.error() was
// populated when it ran.
#[derive(Debug, Default, Clone)]
struct ErrorObserver {
    inner: Arc<ErrorObserverInner>,
}

#[derive(Debug, Default)]
struct ErrorObserverInner {
    after_response_runs: AtomicUsize,
    saw_error: AtomicBool,
}

impl ClientHandler for ErrorObserver {
    async fn after_response(&self, conn: &mut Conn) -> trillium_client::Result<()> {
        self.inner
            .after_response_runs
            .fetch_add(1, Ordering::SeqCst);
        if conn.error().is_some() {
            self.inner.saw_error.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[test(harness)]
async fn after_response_runs_on_transport_error() -> TestResult {
    let observer = ErrorObserver::default();
    let client = Client::new(FailingConnector::new()).with_handler(observer.clone());

    // Transport fails → error propagates from the awaited conn.
    let result = client.get("http://example.com/").await;
    assert!(result.is_err(), "expected transport error, got {result:?}");

    // …but after_response still ran, and saw the stashed error.
    assert_eq!(
        observer.inner.after_response_runs.load(Ordering::SeqCst),
        1,
        "after_response should run exactly once on transport failure"
    );
    assert!(
        observer.inner.saw_error.load(Ordering::SeqCst),
        "after_response should observe the stashed error"
    );
    Ok(())
}

// A handler that synthesizes a recovery response and clears the error,
// causing the awaited conn to return Ok despite the transport failure.
#[derive(Debug)]
struct Recoverer;

impl ClientHandler for Recoverer {
    async fn after_response(&self, conn: &mut Conn) -> trillium_client::Result<()> {
        if conn.take_error().is_some() {
            conn.set_status(Status::Ok).set_response_body("recovered");
        }
        Ok(())
    }
}

#[test(harness)]
async fn after_response_can_recover_from_transport_error() -> TestResult {
    let client = Client::new(FailingConnector::new()).with_handler(Recoverer);

    let mut conn = client.get("http://example.com/").await?;
    assert_eq!(conn.status(), Some(Status::Ok));
    assert_eq!(conn.response_body().read_string().await?, "recovered");
    Ok(())
}

// A handler that queues a follow-up *and* leaves the error stashed —
// i.e. doesn't call `take_error()`. The trampoline should propagate the
// error and discard the queued follow-up.
#[derive(Debug, Default, Clone)]
struct ErroringFollowupQueuer {
    after_response_runs: Arc<AtomicUsize>,
}

impl ClientHandler for ErroringFollowupQueuer {
    async fn after_response(&self, conn: &mut Conn) -> trillium_client::Result<()> {
        self.after_response_runs.fetch_add(1, Ordering::SeqCst);
        if conn.error().is_some() {
            // Don't clear the error; just queue a follow-up. The trampoline should
            // refuse to pick the follow-up up and let the error win.
            let followup = conn.client().get("http://example.com/followup");
            conn.set_followup(followup);
        }
        Ok(())
    }
}

#[test(harness)]
async fn error_wins_over_queued_followup() -> TestResult {
    let handler = ErroringFollowupQueuer::default();
    let client = Client::new(FailingConnector::new()).with_handler(handler.clone());

    let mut conn = client.get("http://example.com/");
    let result = (&mut conn).await;

    assert!(
        result.is_err(),
        "transport error should propagate when after_response leaves it stashed, got {result:?}"
    );
    assert_eq!(
        handler.after_response_runs.load(Ordering::SeqCst),
        1,
        "after_response should run exactly once — the queued follow-up must not be picked up"
    );
    assert!(
        conn.followup().is_none(),
        "trampoline should clear the queued follow-up before propagating the error"
    );
    Ok(())
}
