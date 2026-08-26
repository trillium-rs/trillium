//! Regression tests for the client h1 head-read loop.
//!
//! `read_head` lends windows sized by received bytes, capped by the remaining
//! `head_max_len` allowance, so a server pacing a response head one byte at
//! a time can neither defeat parsing nor drive allocation past the allowance.
//!
//! These tests hold the far end of a duplex [`TestTransport`] and alternate
//! writes with manual polls, mirroring `trillium-http`'s server-side
//! `h1_tests`.

use crate::{Client, Conn};
use std::{
    pin::pin,
    task::{Context, Poll, Waker},
};
use trillium_http::{Buffer, Error};
use trillium_testing::TestTransport;

fn noop_context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

/// A conn wired to one half of a duplex transport; the returned
/// [`TestTransport`] is the far end, standing in for the server.
fn conn_and_server() -> (Conn, TestTransport) {
    let (server, conn_side) = TestTransport::new();
    let mut conn =
        Client::new(trillium_testing::client_config()).build_conn("get", "http://example.test/");
    conn.transport = Some(Box::new(conn_side));
    (conn, server)
}

const RESPONSE_HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

#[test]
fn byte_at_a_time_response_head_still_parses() {
    let (mut conn, server) = conn_and_server();
    let mut cx = noop_context();

    let head_size = {
        let mut head = pin!(conn.read_head());
        let mut ready = None;
        for &byte in RESPONSE_HEAD {
            assert!(
                ready.is_none(),
                "head completed before the full terminator arrived"
            );
            server.write_all([byte]);
            if let Poll::Ready(result) = head.as_mut().poll(&mut cx) {
                ready = Some(result);
            }
        }
        ready
            .expect("head never completed despite the full head arriving")
            .expect("a complete head dripped one byte at a time should parse")
    };

    assert_eq!(head_size, RESPONSE_HEAD.len());
    assert_eq!(&conn.buffer[..], RESPONSE_HEAD);
}

#[test]
fn head_exceeding_head_max_len_errors() {
    let (server, conn_side) = TestTransport::new();
    let mut context = trillium_http::HttpContext::default();
    context.config_mut().set_head_max_len(16);
    let mut client = Client::new(trillium_testing::client_config());
    client.set_context(context);
    let mut conn = client.build_conn("get", "http://example.test/");
    conn.transport = Some(Box::new(conn_side));
    let mut cx = noop_context();

    let mut head = pin!(conn.read_head());
    let mut outcome = None;
    for _ in 0..16 {
        server.write_all(b"A");
        if let Poll::Ready(result) = head.as_mut().poll(&mut cx) {
            outcome = Some(result);
            break;
        }
    }

    assert!(
        matches!(outcome, Some(Err(Error::HeadersTooLong))),
        "expected HeadersTooLong after exhausting the allowance, got {outcome:?}"
    );
}

#[test]
fn eof_before_any_bytes_is_closed() {
    let (mut conn, server) = conn_and_server();
    server.shutdown(std::net::Shutdown::Write);
    let mut cx = noop_context();

    let mut head = pin!(conn.read_head());
    let outcome = head.as_mut().poll(&mut cx);
    assert!(
        matches!(outcome, Poll::Ready(Err(Error::Closed))),
        "expected Closed on immediate eof, got {outcome:?}"
    );
}

#[test]
fn eof_mid_head_is_invalid_head() {
    let (mut conn, server) = conn_and_server();
    server.write_all(b"HTTP/1.1 200 OK\r\n");
    server.shutdown(std::net::Shutdown::Write);
    let mut cx = noop_context();

    let mut head = pin!(conn.read_head());
    let outcome = head.as_mut().poll(&mut cx);
    assert!(
        matches!(outcome, Poll::Ready(Err(Error::InvalidHead))),
        "expected InvalidHead on eof mid-head, got {outcome:?}"
    );
}

/// A previous read may have left a partial head in the buffer — e.g. the
/// expect-continue path. `read_head` must resume from it, including when the
/// terminator spans the prefill/read boundary.
#[test]
fn prefilled_partial_head_resumes_across_the_boundary() {
    let (mut conn, server) = conn_and_server();
    let (prefill, remainder) = RESPONSE_HEAD.split_at(RESPONSE_HEAD.len() - 1);
    conn.buffer = Buffer::from(prefill.to_vec());
    let mut cx = noop_context();

    let head_size = {
        let mut head = pin!(conn.read_head());
        assert!(head.as_mut().poll(&mut cx).is_pending());
        server.write_all(remainder);
        match head.as_mut().poll(&mut cx) {
            Poll::Ready(result) => result.expect("head should parse after the remainder arrives"),
            Poll::Pending => panic!("head should complete once the terminator arrives"),
        }
    };

    assert_eq!(head_size, RESPONSE_HEAD.len());
    assert_eq!(&conn.buffer[..], RESPONSE_HEAD);
}
