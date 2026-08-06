use futures_lite::{Stream, StreamExt, stream};
use std::time::Duration;
use trillium::Conn;
use trillium_server_common::Runtime;
use trillium_sse::{Event, SseConnExt, sse};
use trillium_testing::{TestServer, harness, test};

#[test(harness)]
async fn sse_stream_is_close_delimited_and_well_formed() {
    let app = TestServer::new(|conn: Conn| async move {
        conn.with_sse_stream(stream::iter([
            Event::new("hello"),
            Event::new("world").with_type("greeting"),
        ]))
    })
    .await;

    app.get("/")
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
    let app = TestServer::new(|conn: Conn| async move {
        conn.with_sse_stream(stream::iter([
            Event::new_comment("heartbeat"),
            Event::new_comment(""),
            Event::new("multi\nline").with_id("1").with_comment("note"),
            Event::default(),
            Event::new("last"),
        ]))
    })
    .await;

    app.get("/").await.assert_ok().assert_body(concat!(
        ": heartbeat\n\n",
        ":\n\n",
        ": note\nid: 1\ndata: multi\ndata: line\n\n",
        // an Event with neither data nor a comment is skipped entirely
        "data: last\n\n",
    ));
}

#[test(harness)]
async fn retry_is_emitted_in_milliseconds() {
    let app = TestServer::new(|conn: Conn| async move {
        conn.with_sse_stream(stream::iter([
            Event::default().with_retry(Duration::from_secs(5)),
            Event::new("hello").with_retry(Duration::from_millis(1500)),
        ]))
    })
    .await;

    app.get("/")
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

#[test(harness)]
async fn heartbeats_fill_gaps_between_events() {
    let app = TestServer::new(|conn: Conn| async move {
        let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
        let events = slow_stream(runtime, 1, Duration::from_millis(300));
        conn.with_sse_stream_and_heartbeat(Box::pin(events), Duration::from_millis(20))
    })
    .await;

    assert_heartbeats_then_event(app.get("/").await.assert_ok().body());
}

#[test(harness)]
async fn a_busy_stream_sends_no_heartbeats() {
    let app = TestServer::new(|conn: Conn| async move {
        let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
        let events = slow_stream(runtime, 3, Duration::from_millis(10));
        conn.with_sse_stream_and_heartbeat(Box::pin(events), Duration::from_secs(30))
    })
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_body("data: event 0\n\ndata: event 1\n\ndata: event 2\n\n");
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
        "*/*",
        "text/*",
        "application/json, text/event-stream;q=0.9",
        "*/*; q=0.1",
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
        .assert_body("data: hi\n\n");
}

#[test(harness)]
async fn handler_heartbeat_uses_the_runtime_from_init() {
    let app = TestServer::new(
        sse(|conn: &mut Conn| {
            let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
            Box::pin(slow_stream(runtime, 1, Duration::from_millis(300)))
        })
        .with_heartbeat(Duration::from_millis(20)),
    )
    .await;

    assert_heartbeats_then_event(app.get("/").await.assert_ok().body());
}

#[test(harness)]
async fn handler_without_heartbeat_sends_nothing_extra() {
    let app = TestServer::new(sse(|conn: &mut Conn| {
        let runtime = conn.shared_state::<Runtime>().cloned().unwrap();
        Box::pin(slow_stream(runtime, 1, Duration::from_millis(50)))
    }))
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_body("data: event 0\n\n");
}
