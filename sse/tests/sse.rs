use futures_lite::{AsyncReadExt, AsyncWriteExt, Stream, StreamExt, stream};
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use trillium::Conn;
use trillium_server_common::Runtime;
use trillium_sse::{Event, sse};
use trillium_testing::{RuntimeTrait, TestServer, harness, test, with_transport};

#[test(harness)]
async fn sse_stream_is_close_delimited_and_well_formed() {
    let app = TestServer::new(sse(|_: &mut Conn| {
        stream::iter([
            Event::new("hello"),
            Event::new("world").with_type("greeting"),
        ])
    }))
    .await;

    app.get("/")
        .with_request_header("accept", "text/event-stream")
        .await
        .assert_ok()
        .assert_header("content-type", "text/event-stream")
        .assert_header("cache-control", "no-cache")
        .assert_header("connection", "close")
        // No chunked transfer-encoding leaks into the event stream.
        .assert_no_header("transfer-encoding")
        .assert_body("data: hello\n\nevent: greeting\ndata: world\n\n");
}

#[test(harness)]
async fn comments_ids_and_multiline_data() {
    let app = TestServer::new(sse(|_: &mut Conn| {
        stream::iter([
            Event::new_comment("heartbeat"),
            Event::new_comment(""),
            Event::new("multi\nline").with_id("1").with_comment("note"),
            Event::default(),
            Event::new("last"),
        ])
    }))
    .await;

    app.get("/")
        .with_request_header("accept", "text/event-stream")
        .await
        .assert_ok()
        .assert_body(concat!(
            ": heartbeat\n\n",
            ":\n\n",
            ": note\nid: 1\ndata: multi\ndata: line\n\n",
            // an Event with neither data nor a comment is skipped entirely
            "data: last\n\n",
        ));
}

#[test(harness)]
async fn retry_is_emitted_in_milliseconds() {
    let app = TestServer::new(sse(|_: &mut Conn| {
        stream::iter([
            Event::default().with_retry(Duration::from_secs(5)),
            Event::new("hello").with_retry(Duration::from_millis(1500)),
        ])
    }))
    .await;

    app.get("/")
        .with_request_header("accept", "text/event-stream")
        .await
        .assert_ok()
        .assert_body("retry: 5000\n\nretry: 1500\ndata: hello\n\n");
}

/// A stream that yields `count` events, waiting `every` between each.
fn slow_stream(runtime: Runtime, count: usize, every: Duration) -> impl Stream<Item = Event> {
    stream::iter(0..count).then(move |i| {
        let runtime = runtime.clone();
        Box::pin(async move {
            runtime.delay(every).await;
            Event::new(format!("event {i}"))
        })
    })
}

fn assert_heartbeats_then_event(body: &str) {
    let (heartbeats, event) = body.split_at(body.len() - "data: event 0\n\n".len());
    assert_eq!(event, "data: event 0\n\n");
    assert!(!heartbeats.is_empty(), "expected at least one heartbeat");
    assert!(
        heartbeats
            .split_inclusive("\n\n")
            .all(|beat| beat == ":\n\n"),
        "expected only heartbeats before the event, got {heartbeats:?}"
    );
}

#[test(harness)]
async fn handler_negotiates_on_accept() {
    let app = TestServer::new((sse(|_: &mut Conn| stream::iter(["hi"])), "fallback")).await;

    for accept in [
        "text/event-stream",
        "application/json, text/event-stream;q=0.9",
        "TEXT/EVENT-STREAM",
    ] {
        app.get("/")
            .with_request_header("accept", accept)
            .await
            .assert_ok()
            .assert_body("data: hi\n\n");
    }

    for accept in [
        "application/json",
        "text/html, application/xhtml+xml",
        "*/*",
        "text/*",
        "*/*; q=0.1",
        "*/*;q=0",
        "text/event-stream;q=0.0",
        "",
    ] {
        app.get("/")
            .with_request_header("accept", accept)
            .await
            .assert_ok()
            .assert_body("fallback");
    }

    app.get("/")
        .without_request_header("accept")
        .await
        .assert_ok()
        .assert_body("fallback");
}

#[test(harness)]
async fn heartbeats_fill_gaps_between_events() {
    let app = TestServer::new(
        sse(|conn: &mut Conn| {
            let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
            Box::pin(slow_stream(runtime, 1, Duration::from_millis(300)))
        })
        .with_heartbeat(Duration::from_millis(20)),
    )
    .await;

    assert_heartbeats_then_event(
        app.get("/")
            .with_request_header("accept", "text/event-stream")
            .await
            .assert_ok()
            .body(),
    );
}

#[test(harness)]
async fn a_busy_stream_sends_no_heartbeats() {
    let app = TestServer::new(
        sse(|conn: &mut Conn| {
            let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
            Box::pin(slow_stream(runtime, 3, Duration::from_millis(10)))
        })
        .with_heartbeat(Duration::from_secs(30)),
    )
    .await;

    app.get("/")
        .with_request_header("accept", "text/event-stream")
        .await
        .assert_ok()
        .assert_body("data: event 0\n\ndata: event 1\n\ndata: event 2\n\n");
}

#[test(harness)]
async fn handler_without_heartbeat_sends_nothing_extra() {
    let app = TestServer::new(sse(|conn: &mut Conn| {
        let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
        Box::pin(slow_stream(runtime, 1, Duration::from_millis(50)))
    }))
    .await;

    app.get("/")
        .with_request_header("accept", "text/event-stream")
        .await
        .assert_ok()
        .assert_body("data: event 0\n\n");
}

/// Yields one event, then stays pending forever. Flips `dropped` on drop, so a test can
/// observe that disconnection released the stream.
struct GuardedStream {
    dropped: Arc<AtomicBool>,
    yielded: bool,
}

impl Drop for GuardedStream {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

impl Stream for GuardedStream {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Event>> {
        if self.yielded {
            // deliberately never wakes; the test only needs the stream to hang here
            Poll::Pending
        } else {
            self.yielded = true;
            Poll::Ready(Some(Event::new("hello")))
        }
    }
}

async fn read_until(
    transport: &mut (impl AsyncReadExt + Unpin),
    needle: &str,
) -> std::io::Result<String> {
    let mut received = String::new();
    let mut buf = [0u8; 1024];
    while !received.contains(needle) {
        match transport.read(&mut buf).await? {
            0 => break,
            n => received.push_str(std::str::from_utf8(&buf[..n]).unwrap()),
        }
    }
    Ok(received)
}

#[test]
fn client_disconnection_drops_the_event_stream() {
    let dropped = Arc::new(AtomicBool::new(false));
    let handler = {
        let dropped = dropped.clone();
        sse(move |_: &mut Conn| GuardedStream {
            dropped: dropped.clone(),
            yielded: false,
        })
    };

    with_transport(handler, |mut transport| async move {
        transport
            .write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\naccept: text/event-stream\r\n\r\n")
            .await?;
        read_until(&mut transport, "data: hello\n\n").await?;
        drop(transport);

        let runtime = trillium_testing::runtime();
        for _ in 0..500 {
            if dropped.load(Ordering::Acquire) {
                return Ok(());
            }
            runtime.delay(Duration::from_millis(10)).await;
        }
        panic!("stream was not dropped within 5s of client disconnection");
    });
}

#[test]
fn stray_inbound_bytes_do_not_end_the_stream() {
    // ServerConnector conns don't carry a Runtime in shared state, so build one directly.
    let handler = sse(|_: &mut Conn| {
        Box::pin(slow_stream(
            trillium_testing::runtime().into(),
            2,
            Duration::from_millis(50),
        ))
    });

    with_transport(handler, |mut transport| async move {
        transport
            .write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\naccept: text/event-stream\r\n\r\n")
            .await?;
        let received = read_until(&mut transport, "data: event 0\n\n").await?;
        transport
            .write_all(b"stray bytes the server ignores")
            .await?;
        let received = received
            + read_until(&mut transport, "data: event 1\n\n")
                .await?
                .as_str();
        assert!(received.ends_with("data: event 0\n\ndata: event 1\n\n"));
        Ok(())
    });
}
