use futures_lite::stream;
use trillium::Conn;
use trillium_sse::{Event, SseConnExt};
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
            Event::new_comment("keep-alive"),
            Event::new_comment(""),
            Event::new("multi\nline").with_id("1").with_comment("note"),
            Event::default(),
            Event::new("last"),
        ]))
    })
    .await;

    app.get("/").await.assert_ok().assert_body(concat!(
        ": keep-alive\n\n",
        ":\n\n",
        ": note\nid: 1\ndata: multi\ndata: line\n\n",
        // an Event with neither data nor a comment is skipped entirely
        "data: last\n\n",
    ));
}
