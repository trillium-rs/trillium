use futures_lite::{AsyncRead, io::Cursor};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use test_harness::test;
use trillium_http::{Body, BodySource, Conn, Headers, HttpContext};
use trillium_testing::{HttpTest, RuntimeTrait, TestTransport, harness};

/// A test-only [`BodySource`] that combines a fixed body with a static set of trailers.
///
/// After the body bytes are exhausted, [`BodySource::trailers`] returns the pre-set headers.
struct BodyWithTrailers {
    cursor: Cursor<Vec<u8>>,
    trailers: Option<Headers>,
}

impl BodyWithTrailers {
    fn new(body: impl Into<Vec<u8>>, trailers: Headers) -> Self {
        Self {
            cursor: Cursor::new(body.into()),
            trailers: Some(trailers),
        }
    }
}

impl AsyncRead for BodyWithTrailers {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().cursor).poll_read(cx, buf)
    }
}

impl BodySource for BodyWithTrailers {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        self.get_mut().trailers.take()
    }
}

fn one_trailer(name: &'static str, value: &'static str) -> Headers {
    let mut h = Headers::new();
    h.insert(name, value);
    h
}

#[test(harness)]
async fn server_receives_request_trailers() {
    let test = HttpTest::new(|mut conn: Conn<TestTransport>| async move {
        conn.request_body().read_string().await.unwrap();
        let checksum = conn
            .request_trailers()
            .and_then(|t| t.get_str("x-checksum"))
            .unwrap_or("")
            .to_string();
        conn.set_status(200);
        conn.set_response_body(checksum);
        conn
    });

    let trailers = one_trailer("x-checksum", "abc123");
    test.post("/")
        .with_body(Body::new_with_trailers(
            BodyWithTrailers::new("hello", trailers),
            None,
        ))
        .await
        .assert_ok()
        .assert_body("abc123");
}

#[test(harness)]
async fn server_sends_response_trailers() {
    let test = HttpTest::new(|mut conn: Conn<TestTransport>| async move {
        let trailers = one_trailer("x-checksum", "xyz789");
        conn.set_status(200);
        conn.set_response_body(Body::new_with_trailers(
            BodyWithTrailers::new("hello", trailers),
            None,
        ));
        conn
    });

    let result = test.get("/").await;
    result.assert_ok().assert_body("hello");
    assert_eq!(
        result
            .response_trailers()
            .and_then(|t| t.get_str("x-checksum")),
        Some("xyz789"),
    );
}

#[test(harness)]
async fn bidirectional_trailers() {
    let test = HttpTest::new(|mut conn: Conn<TestTransport>| async move {
        conn.request_body().read_string().await.unwrap();
        let ping = conn
            .request_trailers()
            .and_then(|t| t.get_str("x-ping"))
            .unwrap_or("")
            .to_string();
        let resp_trailers = one_trailer("x-pong", "pong");
        conn.set_status(200);
        conn.set_response_body(Body::new_with_trailers(
            BodyWithTrailers::new(ping, resp_trailers),
            None,
        ));
        conn
    });

    let req_trailers = one_trailer("x-ping", "ping");
    let result = test
        .post("/")
        .with_body(Body::new_with_trailers(
            BodyWithTrailers::new("data", req_trailers),
            None,
        ))
        .await;

    result.assert_ok().assert_body("ping");
    assert_eq!(
        result.response_trailers().and_then(|t| t.get_str("x-pong")),
        Some("pong"),
    );
}

/// Regression: a peer that streams trailer bytes after the last-chunk marker without ever
/// sending the terminator used to grow the connection buffer without bound. The accumulated
/// trailer-section is now capped at `max_header_list_size` — the same bound the h2/h3
/// field-section decoders apply.
#[test(harness)]
async fn oversized_unterminated_trailer_section_is_bounded() {
    let runtime = trillium_testing::runtime();
    let (client, server) = TestTransport::new();
    let handle = runtime.spawn(async move {
        Arc::new(HttpContext::new())
            .run(server, |mut conn: Conn<TestTransport>| async move {
                match conn.request_body().read_string().await {
                    Ok(_) => {
                        conn.set_status(500);
                        conn.set_response_body("body unexpectedly readable");
                    }
                    Err(e) => {
                        conn.set_status(400);
                        conn.set_response_body(format!("{e}"));
                    }
                }
                conn
            })
            .await
    });

    client.write_all(b"POST / HTTP/1.1\r\nHost: _\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n");
    // past the default max_header_list_size of 32 KiB, never terminated
    client.write_all(vec![b'x'; 64 * 1024]);
    client.shutdown(std::net::Shutdown::Write);

    let _ = handle.await.unwrap();
    let response = client.read_available_string().await;
    assert!(
        response.contains("HTTP/1.1 400"),
        "expected a 400 from the capped trailer read: {response:?}"
    );
    assert!(
        response.contains("trailer section too long"),
        "expected the trailer cap error: {response:?}"
    );
}
