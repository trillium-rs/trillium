//! Regression tests for the h1 head-read loop's buffer growth policy.
//!
//! The head loop's window sizing grows geometrically with received bytes, capped
//! by the remaining `head_max_len` allowance. Under the previous policy the
//! buffer was pinned full on every iteration, so each read took the
//! amortized-doubling path even when almost nothing was received — and because
//! the guard bounded accumulated *bytes* rather than capacity, a peer dribbling
//! one byte at a time drove allocation to many times the allowance.
//!
//! These tests hold both halves of a duplex [`TestTransport`] and alternate
//! single-byte writes with polling. The future owns the buffer for its duration,
//! so capacity assertions happen after it has been dropped.

use crate::{Buffer, Conn, Error, HttpContext};
use std::{
    pin::pin,
    task::{Context, Poll, Waker},
};
use trillium_testing::{TestTransport, harness, test};

fn noop_context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

/// Capacity must stay proportional to received bytes plus a bounded read
/// lookahead — never a function of how many times the loop ran. Pre-fix, every
/// dripped byte doubled the allocation; sixteen paced bytes reached gibibytes.
#[test(harness)]
async fn dripped_bytes_do_not_multiply_the_head_buffer() {
    const DRIPS: usize = 16;

    let (client, mut server) = TestTransport::new();
    let context = HttpContext::new();
    let mut buffer = Buffer::with_capacity(32);

    {
        let mut head = pin!(Conn::head(&mut server, &mut buffer, &context));
        let mut cx = noop_context();

        for received in 0..DRIPS {
            client.write_all(b"A");
            assert!(
                matches!(head.as_mut().poll(&mut cx), Poll::Pending),
                "head completed after {received} dripped bytes, before any terminator"
            );
        }
    }

    // One amortized step over the read floor covers {DRIPS} drips. The
    // pre-fix trajectory would exceed this bound many orders of magnitude
    // before the sixteenth byte arrived.
    let ceiling = 2 * (context.config.request_buffer_initial_len + DRIPS);
    assert!(
        buffer.capacity() <= ceiling,
        "{DRIPS} dripped bytes grew the head buffer to {} bytes (ceiling {ceiling})",
        buffer.capacity()
    );
}

/// Byte-at-a-time delivery is legitimate client behavior; the loop must parse a
/// complete head regardless of how finely the peer fragments it.
#[test(harness)]
async fn byte_at_a_time_head_still_parses() {
    let request = b"GET /path?q HTTP/1.1\r\nHost: example\r\nAccept: */*\r\n\r\n";
    let (client, mut server) = TestTransport::new();
    let context = HttpContext::new();
    let mut buffer = Buffer::from(Vec::with_capacity(16));

    client.write_all(request);

    let outcome = {
        let mut head = pin!(Conn::head(&mut server, &mut buffer, &context));
        let mut cx = noop_context();

        let mut ready = None;
        for _ in 0..100 {
            if let Poll::Ready(result) = head.as_mut().poll(&mut cx) {
                ready = Some(result);
                break;
            }
        }
        ready.expect("head never completed despite the full request being available")
    };

    let (head_size, _start_time) =
        outcome.expect("a complete head dripped one byte at a time should parse");
    assert_eq!(head_size, request.len());
    assert_eq!(&buffer[..], &request[..]);
}

/// Dripping non-terminating bytes up to the allowance must surface
/// `HeadersTooLong` with a buffer proportional to that allowance. A deliberately
/// tiny allowance keeps the pre-fix behavior of this test itself small enough to
/// be harmless, while still demonstrating the growth-per-iteration policy.
#[test(harness)]
async fn dribbled_incomplete_head_errors_with_bounded_capacity() {
    let (client, mut server) = TestTransport::new();
    let mut context = HttpContext::new();
    context.config.head_max_len = 16;
    let allowance = context.config.head_max_len;
    let initial_capacity = 64;
    let mut buffer = Buffer::with_capacity(initial_capacity);

    let outcome = {
        let mut head = pin!(Conn::head(&mut server, &mut buffer, &context));
        let mut cx = noop_context();

        let mut ready = None;
        for _ in 0..allowance {
            client.write_all(b"A");
            if let Poll::Ready(result) = head.as_mut().poll(&mut cx) {
                ready = Some(result);
                break;
            }
        }
        ready
    };

    assert!(
        matches!(outcome, Some(Err(Error::HeadersTooLong))),
        "expected HeadersTooLong after {allowance} dribbled bytes, got {outcome:?}"
    );

    // Any sane growth policy stays proportional to the allowance; the unbounded
    // behavior blew past this by orders of magnitude.
    let ceiling = 2 * (allowance + initial_capacity);
    assert!(
        buffer.capacity() <= ceiling,
        "head buffer grew to {} bytes for a {allowance}-byte allowance",
        buffer.capacity()
    );
}
