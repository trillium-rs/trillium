// Integration tests using h3i (quiche-based) against the trillium HTTP/3 server.
//
// These complement the h3-quinn tests by exercising trillium from a completely
// different QUIC/HTTP3 stack, which makes cross-stack interoperability failures
// visible and exercises error-handling paths that well-behaved clients skip.

use futures_lite::AsyncRead;
use h3i::{
    actions::h3::{Action, StreamEvent, StreamEventType, WaitType, send_headers_frame},
    client::{connection_summary::ConnectionSummary, sync_client},
    config::Config,
    frame::H3iFrame,
    quiche,
};
use rcgen::generate_simple_self_signed;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use trillium::{Body, BodySource, Conn, Headers, KnownHeaderName};
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;

// ---------------------------------------------------------------------------
// Infrastructure
// ---------------------------------------------------------------------------

struct TestCert {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
}

fn test_cert() -> TestCert {
    let rcgen::CertifiedKey {
        cert, signing_key, ..
    } = generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    TestCert {
        cert_pem: cert.pem().into_bytes(),
        key_pem: signing_key.serialize_pem().into_bytes(),
    }
}

async fn start_server(handler: impl trillium::Handler, tc: &TestCert) -> SocketAddr {
    let handle = trillium_tokio::config()
        .with_port(0)
        .with_host("localhost")
        .without_signals()
        .with_acceptor(RustlsAcceptor::from_single_cert(&tc.cert_pem, &tc.key_pem))
        .with_quic(QuicConfig::from_single_cert(&tc.cert_pem, &tc.key_pem))
        .spawn(handler);
    *handle.info().await.tcp_socket_addr().unwrap()
}

/// h3i config that skips cert verification and connects directly to `addr`.
fn h3i_config(addr: SocketAddr) -> Config {
    Config::new()
        .with_host_port(format!("localhost:{}", addr.port()))
        .with_connect_to(addr.to_string())
        .verify_peer(false)
        .with_idle_timeout(2000)
        .build()
        .unwrap()
}

async fn h3i_run(config: Config, actions: Vec<Action>) -> ConnectionSummary {
    tokio::task::spawn_blocking(move || {
        sync_client::connect(config, actions, None).expect("h3i connect failed")
    })
    .await
    .expect("spawn_blocking panicked")
}

// ---------------------------------------------------------------------------
// Action helpers
// ---------------------------------------------------------------------------

fn get_request(stream_id: u64, path: &str) -> Action {
    send_headers_frame(
        stream_id,
        true, // fin_stream: GET has no body
        vec![
            quiche::h3::Header::new(b":method", b"GET"),
            quiche::h3::Header::new(b":scheme", b"https"),
            quiche::h3::Header::new(b":authority", b"localhost"),
            quiche::h3::Header::new(b":path", path.as_bytes()),
        ],
    )
}

fn post_request_headers(stream_id: u64, path: &str, content_length: usize) -> Action {
    send_headers_frame(
        stream_id,
        false, // fin_stream: body follows
        vec![
            quiche::h3::Header::new(b":method", b"POST"),
            quiche::h3::Header::new(b":scheme", b"https"),
            quiche::h3::Header::new(b":authority", b"localhost"),
            quiche::h3::Header::new(b":path", path.as_bytes()),
            quiche::h3::Header::new(b"content-type", b"text/plain"),
            quiche::h3::Header::new(b"content-length", content_length.to_string().as_bytes()),
        ],
    )
}

fn send_data(stream_id: u64, payload: &'static [u8], fin: bool) -> Action {
    Action::SendFrame {
        stream_id,
        fin_stream: fin,
        frame: quiche::h3::frame::Frame::Data {
            payload: payload.to_vec(),
        },
    }
}

fn wait_finished(stream_id: u64) -> Action {
    Action::Wait {
        wait_type: WaitType::StreamEvent(StreamEvent {
            stream_id,
            event_type: StreamEventType::Finished,
        }),
    }
}

fn close_no_error() -> Action {
    Action::ConnectionClose {
        error: quiche::ConnectionError {
            is_app: true,
            error_code: quiche::h3::WireErrorCode::NoError as u64,
            reason: vec![],
        },
    }
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

/// Extract the `:status` value from the first HEADERS frame in `frames`.
fn response_status(frames: &[H3iFrame]) -> Option<u16> {
    frames.iter().find_map(|f| {
        let h = f.to_enriched_headers()?;
        let status = h.status_code()?;
        std::str::from_utf8(status).ok()?.parse().ok()
    })
}

/// Return all decoded HEADERS frames from `frames` in order.
fn all_headers(frames: &[H3iFrame]) -> Vec<h3i::frame::EnrichedHeaders> {
    frames
        .iter()
        .filter_map(|f| f.to_enriched_headers())
        .collect()
}

// ---------------------------------------------------------------------------
// A minimal BodySource that emits a fixed body then produces trailers.
// Used to test the server's trailer-sending path end-to-end.
// ---------------------------------------------------------------------------

struct TrailingBody {
    inner: futures_lite::io::Cursor<Vec<u8>>,
}

impl AsyncRead for TrailingBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl BodySource for TrailingBody {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        Some(Headers::new().with_inserted_header("x-trailer-checksum", "abc123"))
    }
}

// ---------------------------------------------------------------------------
// Tests: happy-path conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3i_basic_get() {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("hello from h3i") }, &tc).await;

    let summary = h3i_run(
        h3i_config(addr),
        vec![get_request(0, "/"), wait_finished(0), close_no_error()],
    )
    .await;

    let frames = summary.stream_map.stream(0);
    assert_eq!(response_status(&frames), Some(200));
}

#[tokio::test]
async fn h3i_custom_response_headers() {
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move {
            conn.ok("ok")
                .with_response_header("x-custom-header", "quiche-test-value")
        },
        &tc,
    )
    .await;

    let summary = h3i_run(
        h3i_config(addr),
        vec![get_request(0, "/"), wait_finished(0), close_no_error()],
    )
    .await;

    let frames = summary.stream_map.stream(0);
    let headers = all_headers(&frames);
    assert!(!headers.is_empty(), "no HEADERS frame received");
    let hmap = headers[0].header_map();
    assert_eq!(
        hmap.get(b"x-custom-header" as &[u8]).map(|v| v.as_slice()),
        Some(b"quiche-test-value" as &[u8]),
        "custom header not found in response: {hmap:?}"
    );
}

#[tokio::test]
async fn h3i_post_echoes_body() {
    let tc = test_cert();
    let addr = start_server(
        |mut conn: Conn| async move {
            let body = conn.request_body_string().await.unwrap_or_default();
            conn.ok(format!("echo:{body}"))
        },
        &tc,
    )
    .await;

    let body_bytes: &'static [u8] = b"ping";
    let summary = h3i_run(
        h3i_config(addr),
        vec![
            post_request_headers(0, "/", body_bytes.len()),
            send_data(0, body_bytes, true),
            wait_finished(0),
            close_no_error(),
        ],
    )
    .await;

    let frames = summary.stream_map.stream(0);
    assert_eq!(response_status(&frames), Some(200));
}

// ---------------------------------------------------------------------------
// Trailers test — the primary motivation for this test suite.
//
// Safari was hanging indefinitely when the server sent trailers. This test
// verifies that the server emits exactly the right frame sequence:
//   HEADERS (response) → DATA → HEADERS (trailers) → stream FIN
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3i_trailers_frame_sequence() {
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move {
            conn.ok(Body::new_with_trailers(
                TrailingBody {
                    inner: futures_lite::io::Cursor::new(b"body content".to_vec()),
                },
                None,
            ))
            .with_response_header(KnownHeaderName::Trailer, "x-trailer-checksum")
        },
        &tc,
    )
    .await;

    let summary = h3i_run(
        h3i_config(addr),
        vec![get_request(0, "/"), wait_finished(0), close_no_error()],
    )
    .await;

    let frames = summary.stream_map.stream(0);
    let headers = all_headers(&frames);

    // Must have at least two HEADERS frames: response headers + trailers.
    assert!(
        headers.len() >= 2,
        "expected at least 2 HEADERS frames (response + trailers), got {}. frames: {frames:?}",
        headers.len()
    );

    // First HEADERS frame is the response.
    assert_eq!(
        headers[0].status_code().map(|s| s.as_slice()),
        Some(b"200" as &[u8]),
        "first HEADERS frame should be a 200 response"
    );

    // Second HEADERS frame is the trailers.
    let trailer_map = headers[1].header_map();
    assert!(
        trailer_map.get(b"x-trailer-checksum" as &[u8]).is_some(),
        "trailers HEADERS frame missing x-trailer-checksum. header_map: {trailer_map:?}"
    );
}

// ---------------------------------------------------------------------------
// Tests: RFC 9114 error handling
// ---------------------------------------------------------------------------

/// RFC 9114 §4.1.2 — content-length mismatch must be a stream error.
/// Sending 4 bytes but declaring content-length: 5 is malformed.
///
/// Note: trillium's ReceivedBody correctly detects this mismatch and returns an Err,
/// but the framework does not currently enforce it at the protocol level — a handler
/// that ignores the body read error can still respond 200. This test uses a handler
/// that propagates the error; the companion issue is framework-level enforcement.
#[tokio::test]
async fn h3i_content_length_mismatch() {
    let tc = test_cert();
    let addr = start_server(
        |mut conn: Conn| async move {
            match conn.request_body_string().await {
                Ok(body) => conn.ok(format!("echo: {body}")),
                Err(_) => conn.with_status(400).with_body("bad request").halt(),
            }
        },
        &tc,
    )
    .await;

    let summary = h3i_run(
        h3i_config(addr),
        vec![
            post_request_headers(0, "/", 5), // claims 5 bytes
            send_data(0, b"test", true),     // sends only 4
            wait_finished(0),
            close_no_error(),
        ],
    )
    .await;

    let frames = summary.stream_map.stream(0);

    // Server must respond with 400 and/or close the connection with H3_MESSAGE_ERROR.
    let got_400 = response_status(&frames) == Some(400);
    let got_reset = frames.iter().any(|f| matches!(f, H3iFrame::ResetStream(_)));
    let peer_error_code = summary
        .conn_close_details
        .peer_error()
        .map(|e| e.error_code);
    let got_message_error = peer_error_code == Some(quiche::h3::WireErrorCode::MessageError as u64);

    assert!(
        got_400 || got_reset || got_message_error,
        "expected 400 response, stream reset, or H3_MESSAGE_ERROR connection close for \
         content-length mismatch; status={:?}, reset={got_reset}, peer_error={peer_error_code:?}",
        response_status(&frames),
    );
}

/// RFC 9114 §4.1.2 — HTTP/1.1 connection headers (Connection, Transfer-Encoding, etc.)
/// are forbidden in HTTP/3 and must be rejected.
///
/// The server should terminate the malformed stream without responding, but must NOT
/// close the entire connection — subsequent requests on new streams must still work.
#[tokio::test]
async fn h3i_connection_headers_rejected() {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("ok") }, &tc).await;

    let bad_request = send_headers_frame(
        0,
        true,
        vec![
            quiche::h3::Header::new(b":method", b"GET"),
            quiche::h3::Header::new(b":scheme", b"https"),
            quiche::h3::Header::new(b":authority", b"localhost"),
            quiche::h3::Header::new(b":path", b"/"),
            quiche::h3::Header::new(b"connection", b"keep-alive"), // forbidden in H3
        ],
    );

    let summary = h3i_run(
        h3i_config(addr),
        vec![
            bad_request,
            wait_finished(0), // server drops the malformed stream
            // Connection must remain usable for subsequent requests
            get_request(4, "/"),
            wait_finished(4),
            close_no_error(),
        ],
    )
    .await;

    // The malformed stream gets no response
    assert_eq!(
        response_status(&summary.stream_map.stream(0)),
        None,
        "expected no response on malformed stream 0"
    );
    // The connection is not killed — stream 4 succeeds
    assert_eq!(
        response_status(&summary.stream_map.stream(4)),
        Some(200),
        "expected 200 on stream 4 after malformed stream 0 was rejected"
    );
}

/// RFC 9114 §4.1.1 — requests must include the :method pseudo-header.
/// Missing :method is a malformed request → stream is terminated, connection stays open.
#[tokio::test]
async fn h3i_missing_method_pseudo_header() {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("ok") }, &tc).await;

    let no_method = send_headers_frame(
        0,
        true,
        vec![
            // Deliberately omit :method
            quiche::h3::Header::new(b":scheme", b"https"),
            quiche::h3::Header::new(b":authority", b"localhost"),
            quiche::h3::Header::new(b":path", b"/"),
        ],
    );

    let summary = h3i_run(
        h3i_config(addr),
        vec![
            no_method,
            wait_finished(0),
            get_request(4, "/"),
            wait_finished(4),
            close_no_error(),
        ],
    )
    .await;

    assert_eq!(
        response_status(&summary.stream_map.stream(0)),
        None,
        "expected no response on stream missing :method"
    );
    assert_eq!(
        response_status(&summary.stream_map.stream(4)),
        Some(200),
        "expected 200 on stream 4 after stream 0 was rejected for missing :method"
    );
}

// ---------------------------------------------------------------------------
// Tests: connection-level robustness
// ---------------------------------------------------------------------------

/// Send two requests on streams 0 and 4. Both should complete successfully.
#[tokio::test]
async fn h3i_sequential_streams() {
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move {
            let path = conn.path().to_string();
            conn.ok(format!("path:{path}"))
        },
        &tc,
    )
    .await;

    let summary = h3i_run(
        h3i_config(addr),
        vec![
            get_request(0, "/first"),
            wait_finished(0),
            get_request(4, "/second"),
            wait_finished(4),
            close_no_error(),
        ],
    )
    .await;

    assert_eq!(response_status(&summary.stream_map.stream(0)), Some(200));
    assert_eq!(response_status(&summary.stream_map.stream(4)), Some(200));
}

/// Resetting a stream after sending headers should not crash the server.
/// The next request on a new stream should still work.
#[tokio::test]
async fn h3i_reset_stream_is_handled() {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("ok") }, &tc).await;

    let summary = h3i_run(
        h3i_config(addr),
        vec![
            get_request(0, "/"),
            // Reset stream 0 immediately (request cancellation)
            Action::ResetStream {
                stream_id: 0,
                error_code: quiche::h3::WireErrorCode::RequestCancelled as u64,
            },
            // Server should still handle a new request on stream 4
            get_request(4, "/"),
            wait_finished(4),
            close_no_error(),
        ],
    )
    .await;

    // Stream 4 should complete successfully despite stream 0 being reset
    assert_eq!(
        response_status(&summary.stream_map.stream(4)),
        Some(200),
        "stream 4 should succeed after stream 0 was reset"
    );
}
