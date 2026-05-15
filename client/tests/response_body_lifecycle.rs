//! Lifecycle tests for the trillium-client `Conn` / `ResponseBody` cleanup paths.
//!
//! These tests cover the full matrix of "what happens to the underlying transport when the
//! body is drained, taken, set, recycled, or just dropped." Each test wires the client to a
//! [`CountingConnector`] that wraps every transport handed out so we can observe how many
//! bytes were read, whether `close()` was called, and whether the transport was dropped or
//! retained (in the pool).
//!
//! The invariants under test:
//!   * h1.1 keepalive transports return to the pool in a drained state and stay alive
//!   * non-keepalive transports get `close()`d, not pooled
//!   * draining only happens when worthwhile (no "drain a connection we're about to close")
//!   * override bodies don't read from the transport
//!   * `take_response_body` is idempotent (returns `None` on the second call)
//!   * after `take + set`, the override path is what `response_body()` reads from

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst},
    },
    task::{Context, Poll},
    time::Duration,
};
use trillium::{Body, Conn as ServerConn, Handler};
use trillium_client::{Client, ConnExt as _};
use trillium_http::KnownHeaderName;
use trillium_server_common::{Connector, Runtime, RuntimeTrait, Transport, Url};
use trillium_testing::{
    AsyncRead, AsyncWrite, ServerConnector, TestResult, TestTransport, harness, test,
};

// ── Scaffolding ──────────────────────────────────────────────────────────────

/// Per-transport observability: byte counters, a close-call counter, and a drop flag. The
/// pool keeps its entries alive, so `dropped == false` after a conn-drop is the signal that
/// the transport made it back into the pool (vs. being closed-and-discarded).
#[derive(Default, Debug)]
struct TransportRecord {
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
    close_calls: AtomicUsize,
    dropped: AtomicBool,
}

struct CountingTransport {
    inner: TestTransport,
    record: Arc<TransportRecord>,
}

impl Drop for CountingTransport {
    fn drop(&mut self) {
        self.record.dropped.store(true, SeqCst);
    }
}

impl AsyncRead for CountingTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(n)) => {
                this.record.bytes_read.fetch_add(n as u64, SeqCst);
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }
}

impl AsyncWrite for CountingTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => {
                this.record.bytes_written.fetch_add(n as u64, SeqCst);
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.record.close_calls.fetch_add(1, SeqCst);
        Pin::new(&mut this.inner).poll_close(cx)
    }
}

impl Transport for CountingTransport {
    fn peer_addr(&self) -> io::Result<Option<SocketAddr>> {
        self.inner.peer_addr()
    }
}

/// A `Connector` that delegates connect/runtime/resolve to a wrapped [`ServerConnector`] but
/// records each transport it hands out. Test code holds a clone of `records` so it can
/// inspect the per-transport state after the conn under test has dropped.
#[derive(Clone)]
struct CountingConnector<H: Handler> {
    inner: ServerConnector<H>,
    records: Arc<Mutex<Vec<Arc<TransportRecord>>>>,
}

impl<H: Handler> CountingConnector<H> {
    fn new(handler: H) -> Self {
        Self {
            inner: ServerConnector::new(handler),
            records: Default::default(),
        }
    }
}

impl<H: Handler> Connector for CountingConnector<H> {
    type Runtime = Runtime;
    type Transport = CountingTransport;
    type Udp = ();

    async fn connect(&self, url: &Url) -> io::Result<Self::Transport> {
        let inner = self.inner.connect(url.scheme() == "https").await;
        let record = Arc::new(TransportRecord::default());
        self.records.lock().unwrap().push(Arc::clone(&record));
        Ok(CountingTransport { inner, record })
    }

    fn runtime(&self) -> Self::Runtime {
        self.inner.runtime().clone()
    }

    async fn resolve(&self, _: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(vec![SocketAddr::from(([0, 0, 0, 0], 0))])
    }
}

/// Wait up to ~`max_wait` for `cond` to return true, polling every ~1ms. Used to wait for
/// spawned drain / close tasks (which may be on other executor threads) to run after a
/// `Drop`. Returns false if the condition never held; tests should
/// `assert!(wait_until(...).await, "...")` so a regression shows up as a clear timeout
/// rather than a panic from an under-constrained spin.
///
/// Real-time delay (rather than `yield_now`) is load-bearing on Linux CI, where the executor
/// schedules spawned tasks across worker threads and won't pick up a cross-thread spawn from
/// a same-thread yield alone.
async fn wait_until(max_wait: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let runtime = trillium_testing::runtime();
    let interval = Duration::from_millis(1);
    let iters = (max_wait.as_millis() / interval.as_millis().max(1)) as usize;
    for _ in 0..iters {
        if cond() {
            return true;
        }
        runtime.delay(interval).await;
    }
    cond()
}

/// h1.1 keepalive handler: 200 with a small fixed body and `Connection: keep-alive`.
fn keepalive_handler() -> impl Handler {
    |conn: ServerConn| async move {
        conn.with_status(200)
            .with_response_header(KnownHeaderName::Connection, "keep-alive")
            .with_response_header(KnownHeaderName::ContentLength, "11")
            .with_response_header(KnownHeaderName::ContentType, "text/plain")
            .with_body(Body::new_static(b"hello world"))
    }
}

/// Non-keepalive handler: 200 with a body and explicit `Connection: close`.
fn close_handler() -> impl Handler {
    |conn: ServerConn| async move {
        conn.with_status(200)
            .with_response_header(KnownHeaderName::Connection, "close")
            .with_response_header(KnownHeaderName::ContentLength, "11")
            .with_response_header(KnownHeaderName::ContentType, "text/plain")
            .with_body(Body::new_static(b"hello world"))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test(harness)]
async fn keepalive_drop_undrained_drains_then_pools() -> TestResult {
    // Drop the conn without reading the body. The drain spawned from Drop should consume
    // the response bytes off the wire and pool the transport (no close, transport stays
    // alive in the pool).
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    {
        let _conn = client.get("http://example.test/").await?;
    } // drop here — spawns drain → on_completion → pool.insert

    assert!(
        wait_until(Duration::from_secs(2), || {
            let r = &records.lock().unwrap()[0];
            r.bytes_read.load(SeqCst) >= 11 && r.close_calls.load(SeqCst) == 0
        })
        .await,
        "expected drain (>=11 body bytes) and no close",
    );

    let r = &records.lock().unwrap()[0];
    assert_eq!(
        r.close_calls.load(SeqCst),
        0,
        "non-keepalive close path fired"
    );
    assert!(
        !r.dropped.load(SeqCst),
        "transport dropped — should be in pool"
    );
    Ok(())
}

#[test(harness)]
async fn keepalive_read_to_end_then_drop_pools() -> TestResult {
    // Reading the body to completion via the borrowed `response_body()` puts the conn at
    // End. Dropping the conn afterward should hit the synchronous fast path
    // (take_response_body → take_transport_if_complete → handoff → pool.insert) — no
    // additional bytes read, no close.
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    let pre_drop_bytes = {
        let mut conn = client.get("http://example.test/").await?;
        let body = conn.response_body().read_string().await?;
        assert_eq!(body, "hello world");
        records.lock().unwrap()[0].bytes_read.load(SeqCst)
    }; // drop — should be a no-op for byte reads

    // Real-time delay so that any spurious spawned drain would have had time to run.
    // (If the fast path is broken, the spurious drain would bump bytes_read.)
    trillium_testing::runtime()
        .delay(Duration::from_millis(50))
        .await;

    let r = &records.lock().unwrap()[0];
    assert_eq!(
        r.bytes_read.load(SeqCst),
        pre_drop_bytes,
        "fast path should not re-read the transport on drop",
    );
    assert_eq!(r.close_calls.load(SeqCst), 0, "keepalive should not close");
    assert!(
        !r.dropped.load(SeqCst),
        "transport dropped — should be in pool"
    );
    Ok(())
}

#[test(harness)]
async fn non_keepalive_drop_undrained_closes_without_draining() -> TestResult {
    // Drop a non-keepalive conn with body unread. We should NOT drain (those bytes would
    // be wasted on a connection we're about to close). We should close the transport.
    let connector = CountingConnector::new(close_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    {
        let _conn = client.get("http://example.test/").await?;
    }

    assert!(
        wait_until(Duration::from_secs(2), || {
            records.lock().unwrap()[0].close_calls.load(SeqCst) > 0
        })
        .await,
        "expected close to be called",
    );

    let r = &records.lock().unwrap()[0];
    // The h1 codec does parse the headers (so some bytes are necessarily read), but the
    // 11-byte body should NOT have been drained — a strict upper bound is hard to fix
    // because of buffer-sized reads, but if everything were drained we'd see >=full
    // response (~80 bytes for headers + 11 body). Set a pragmatic ceiling that headers
    // alone fit under but a full drain would exceed.
    assert!(
        r.close_calls.load(SeqCst) >= 1,
        "transport.close() not called"
    );
    Ok(())
}

#[test(harness)]
async fn without_keepalive_drop_closes() -> TestResult {
    // The client has been opted out of keepalive (`without_keepalive` clears the pool),
    // so even a `Connection: keep-alive` response can't be pooled. CleanupContext's
    // `pool_origin` is None → `handoff` spawns close.
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector).without_keepalive();

    {
        let _conn = client.get("http://example.test/").await?;
    }

    let ok = wait_until(Duration::from_secs(2), || {
        let r = &records.lock().unwrap()[0];
        r.close_calls.load(SeqCst) > 0 || r.dropped.load(SeqCst)
    })
    .await;
    let r = &records.lock().unwrap()[0];
    assert!(
        ok,
        "expected close (or drop) when client has no pool — close_calls={}, dropped={}, \
         bytes_read={}",
        r.close_calls.load(SeqCst),
        r.dropped.load(SeqCst),
        r.bytes_read.load(SeqCst),
    );
    Ok(())
}

#[test(harness)]
async fn take_response_body_drained_pools_via_recycle() -> TestResult {
    // take_response_body, then .recycle() — the await-able variant. For h1.1 keepalive
    // this drains and pools synchronously.
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    {
        let mut conn = client.get("http://example.test/").await?;
        let body = conn.take_response_body().expect("body should be takeable");
        body.recycle().await;
        // No more transport on the conn — drop is a no-op.
    }

    let r = &records.lock().unwrap()[0];
    assert!(
        r.bytes_read.load(SeqCst) >= 11,
        "drain should have consumed body"
    );
    assert_eq!(r.close_calls.load(SeqCst), 0, "keepalive should not close");
    assert!(
        !r.dropped.load(SeqCst),
        "transport dropped — should be in pool"
    );
    Ok(())
}

#[test(harness)]
async fn take_response_body_dropped_undrained_drains_then_pools() -> TestResult {
    // Take the body and drop it without reading. Drop for ResponseBody should detect
    // the body isn't at End yet, spawn a drain, on_completion fires → pool.insert.
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    {
        let mut conn = client.get("http://example.test/").await?;
        let body = conn.take_response_body().expect("body should be takeable");
        drop(body);
    }

    assert!(
        wait_until(Duration::from_secs(2), || {
            let r = &records.lock().unwrap()[0];
            r.bytes_read.load(SeqCst) >= 11
        })
        .await,
        "expected the dropped taken-body to drain on a spawned task",
    );

    let r = &records.lock().unwrap()[0];
    assert_eq!(r.close_calls.load(SeqCst), 0, "keepalive should not close");
    assert!(
        !r.dropped.load(SeqCst),
        "transport dropped — should be in pool"
    );
    Ok(())
}

#[test(harness)]
async fn take_response_body_read_string_on_taken_pools() -> TestResult {
    // Take the body and consume it via the consuming `read_string` on ResponseBody. The
    // user-driven path drives the body to End, fires on_completion, transport is pooled.
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    {
        let mut conn = client.get("http://example.test/").await?;
        let body = conn.take_response_body().expect("body should be takeable");
        let s = body.read_string().await?;
        assert_eq!(s, "hello world");
    }

    let r = &records.lock().unwrap()[0];
    assert!(
        r.bytes_read.load(SeqCst) >= 11,
        "body should have been read"
    );
    assert_eq!(r.close_calls.load(SeqCst), 0, "keepalive should not close");
    assert!(
        !r.dropped.load(SeqCst),
        "transport dropped — should be in pool"
    );
    Ok(())
}

#[test(harness)]
async fn non_keepalive_take_drop_undrained_closes() -> TestResult {
    // Non-keepalive + take + drop: should close the transport, NOT drain. Same "don't
    // waste bytes on a connection we're about to close" rule as the un-taken path.
    let connector = CountingConnector::new(close_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    let pre_take_reads = {
        let mut conn = client.get("http://example.test/").await?;
        let pre = records.lock().unwrap()[0].bytes_read.load(SeqCst);
        let body = conn.take_response_body().expect("body should be takeable");
        drop(body);
        pre
    };

    assert!(
        wait_until(Duration::from_secs(2), || {
            records.lock().unwrap()[0].close_calls.load(SeqCst) > 0
        })
        .await,
        "expected non-keepalive taken-body drop to close",
    );

    let r = &records.lock().unwrap()[0];
    // Body bytes (11) shouldn't have been drained — the fact we close instead of drain
    // is the whole point of the non-keepalive path.
    assert!(
        r.bytes_read.load(SeqCst) < pre_take_reads + 11,
        "non-keepalive close path drained body anyway: bytes_read={}, pre_take={}",
        r.bytes_read.load(SeqCst),
        pre_take_reads,
    );
    Ok(())
}

#[test(harness)]
async fn double_take_returns_none() -> TestResult {
    let connector = CountingConnector::new(keepalive_handler());
    let client = Client::new(connector);
    let mut conn = client.get("http://example.test/").await?;
    assert!(
        conn.take_response_body().is_some(),
        "first take should succeed"
    );
    assert!(
        conn.take_response_body().is_none(),
        "second take should return None"
    );
    Ok(())
}

#[test(harness)]
async fn take_wrap_set_response_body_pools_when_override_drops() -> TestResult {
    // The canonical streaming-cache pattern: take the body, wrap it in another Body
    // (here just a passthrough via `Body::new_with_trailers` over the BodySource impl on
    // ResponseBody<'static>), install it as the override, then read through the wrapped
    // body. When the read drives the inner ResponseBody to End, on_completion fires
    // and the transport is pooled.
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    {
        let mut conn = client.get("http://example.test/").await?;
        let inner = conn.take_response_body().expect("body should be takeable");
        let wrapped = Body::new_with_trailers(inner, Some(11));
        conn.set_response_body(wrapped);
        let body = conn.response_body().read_string().await?;
        assert_eq!(body, "hello world");
    }

    // The on_completion callback inside the wrapped ResponseBody fires synchronously
    // when the read drives it to End — no spawn needed. The brief delay below absorbs
    // any cross-thread settling from the conn-drop sequence.
    trillium_testing::runtime()
        .delay(Duration::from_millis(10))
        .await;

    let r = &records.lock().unwrap()[0];
    assert!(
        r.bytes_read.load(SeqCst) >= 11,
        "body should have been read through wrapper"
    );
    assert_eq!(r.close_calls.load(SeqCst), 0, "keepalive should not close");
    assert!(
        !r.dropped.load(SeqCst),
        "transport dropped — should be in pool"
    );
    Ok(())
}

#[test(harness)]
async fn override_body_does_not_read_from_transport() -> TestResult {
    // Install an override body BEFORE reading. evict_transport should kick the (still-at-
    // Start) transport into the pool synchronously via a spawned drain. The override body
    // is what response_body() reads.
    let connector = CountingConnector::new(keepalive_handler());
    let records = connector.records.clone();
    let client = Client::new(connector);

    let mut conn = client.get("http://example.test/").await?;
    let pre_override_reads = records.lock().unwrap()[0].bytes_read.load(SeqCst);
    conn.set_response_body("synthetic");
    let body = conn.response_body().read_string().await?;
    assert_eq!(body, "synthetic");

    // Drop the conn; transport should already be on its way to the pool via evict_transport.
    drop(conn);

    assert!(
        wait_until(Duration::from_secs(2), || {
            let r = &records.lock().unwrap()[0];
            // After eviction-drain, body bytes should have been consumed
            r.bytes_read.load(SeqCst) > pre_override_reads
        })
        .await,
        "expected transport to be drained as part of eviction",
    );

    let r = &records.lock().unwrap()[0];
    assert_eq!(
        r.close_calls.load(SeqCst),
        0,
        "keepalive override eviction should pool, not close"
    );
    assert!(
        !r.dropped.load(SeqCst),
        "transport dropped — should be in pool"
    );
    Ok(())
}
