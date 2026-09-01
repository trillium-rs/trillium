//! How much work one `drive` call gets done.
//!
//! Every other suite here asks whether the driver emits the *right* bytes. This one asks how
//! many `drive` calls it takes to emit them, because that is the quantity a saturated server
//! actually pays: `drive` returns `Pending` after `copy_loops_per_yield` rounds and re-arms via
//! `wake_by_ref`, so each exhausted budget costs a full runtime reschedule. A driver that emits
//! 8 KB per reschedule and one that emits 512 KB do the same protocol work and have very
//! different throughput.
//!
//! Sized against the arena's `static-h2` profile, which is where this was noticed: 32
//! concurrent streams per connection, ~15.7 KB per response (the precompressed sidecars
//! `trillium-static` serves under `Accept-Encoding: br`), bodies streamed from a source rather
//! than held in memory.
//!
//! These assert *relationships*, not absolute numbers — the point is which knob moves the work
//! done per reschedule, and that the answer does not silently change.

use super::fixture::*;
use crate::{
    Body, BodySource, Headers, HttpConfig, Method, Status,
    h2::{frame::Frame, settings::H2Settings},
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

/// Bytes per response. The arena's `static-h2` runs measure 15.7 KB/req across the 20-file
/// rotation, so this is that, rounded.
const RESPONSE_LEN: usize = 15_700;

/// Concurrent streams per connection — `h2load -m 32`, what the profile runs.
const STREAMS: u32 = 32;

/// A body large enough that one `drive` call cannot possibly carry it, for locating the regime
/// where the yield budget starts to bind at all.
const LARGE_RESPONSE_LEN: usize = 4 * 1024 * 1024;

/// A streaming body of a fixed length, modeling a file read rather than an in-memory buffer —
/// `Body::new_static` would take the whole-body write path and skip the chunked framing the
/// static handler actually exercises.
struct FixedBody {
    remaining: usize,
}

impl AsyncRead for FixedBody {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let n = this.remaining.min(buf.len());
        buf[..n].fill(b'x');
        this.remaining -= n;
        Poll::Ready(Ok(n))
    }
}

impl BodySource for FixedBody {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        None
    }
}

#[derive(Debug)]
struct DrainStats {
    /// `drive` calls needed to put every response on the wire. Each one that exhausts its
    /// budget is a runtime reschedule.
    ticks: usize,
    data_bytes: u64,
    data_frames: usize,
    /// Largest DATA payload the driver emitted, for comparing against the peer's advertised
    /// `SETTINGS_MAX_FRAME_SIZE` and against our own `h2_max_frame_size`.
    max_frame_payload: u32,
}

impl DrainStats {
    fn bytes_per_tick(&self) -> u64 {
        self.data_bytes / self.ticks.max(1) as u64
    }
}

/// Open `STREAMS` request streams, stage a `RESPONSE_LEN` streaming response on each, then tick
/// until the wire goes quiet, counting what each tick bought.
fn drain_responses(config: HttpConfig, response_len: usize) -> DrainStats {
    // Enough window that flow control is never the binding constraint. Without it the peer's
    // RFC-default 65535 connection window stalls the send pump long before the yield budget
    // does, and this would measure flow control instead of what it means to.
    let window = u32::try_from(response_len * STREAMS as usize * 2).expect("window fits u32");

    let mut fx = DriverFixture::new_server_with_config(config);
    // Peer settings, not ours: outbound DATA framing is bounded by what the *peer* advertises
    // it will accept, so this is the knob that governs the frame sizes we emit.
    fx.complete_handshake_with_peer_settings(
        H2Settings::default().with_initial_window_size(window),
    );
    fx.peer_window_update(0, window);

    let stream_ids: Vec<u32> = (0..STREAMS).map(|i| 1 + i * 2).collect();

    for &id in &stream_ids {
        fx.peer_open_stream(id, Method::Get, "/static/app.js", true);
    }
    // Each yielded Conn is one request handed to the handler; drain them all before staging
    // responses so every stream is contending at once, the way the profile has it. They have to
    // be kept alive — dropping a Conn tears its stream down, and the responses go nowhere.
    let mut conns = Vec::new();
    for _ in 0..stream_ids.len() * 2 {
        if let Poll::Ready(Some(Ok(conn))) = fx.tick() {
            conns.push(conn);
        }
    }
    assert_eq!(
        conns.len(),
        stream_ids.len(),
        "expected one Conn per opened stream",
    );
    let _ = fx.next_outbound_bytes();

    // Submission handles, like the Conns, have to outlive the drain loop.
    let _submits: Vec<_> = stream_ids
        .iter()
        .map(|&id| {
            let body = Body::new_with_trailers(
                FixedBody {
                    remaining: response_len,
                },
                Some(response_len as u64),
            );
            let pseudos = PseudoHeaders::default().with_status(Status::Ok);
            fx.connection
                .submit_send(id, pseudos, Headers::new(), Some(body))
        })
        .collect();

    let expected = response_len as u64 * u64::from(STREAMS);
    let mut stats = DrainStats {
        ticks: 0,
        data_bytes: 0,
        data_frames: 0,
        max_frame_payload: 0,
    };

    // Generous ceiling; the assertions below catch a drain that never completes.
    for _ in 0..4096 {
        if stats.data_bytes >= expected {
            break;
        }
        let _ = fx.tick();
        stats.ticks += 1;
        for frame in fx.next_outbound_frames() {
            if let Frame::Data { data_length, .. } = frame {
                stats.data_bytes += u64::from(data_length);
                stats.data_frames += 1;
                stats.max_frame_payload = stats.max_frame_payload.max(data_length);
            }
        }
    }

    assert_eq!(
        stats.data_bytes, expected,
        "drain did not complete: {stats:?} (expected {expected} body bytes)",
    );
    stats
}

/// The headline measurement. Printed rather than pinned to a number, because the interesting
/// output is the comparison across configs — run with `--nocapture`.
#[test]
fn work_per_drive_call_across_configs() {
    let cases = [
        ("defaults", HttpConfig::default()),
        (
            "copy_loops=4",
            HttpConfig::default().with_copy_loops_per_yield(4),
        ),
        (
            "copy_loops=64",
            HttpConfig::default().with_copy_loops_per_yield(64),
        ),
        (
            "copy_loops=256",
            HttpConfig::default().with_copy_loops_per_yield(256),
        ),
        (
            "h2_max_frame_size=64K",
            HttpConfig::default().with_h2_max_frame_size(64 * 1024),
        ),
        (
            "body_write_chunk_len=64K",
            HttpConfig::default().with_body_write_chunk_len(64 * 1024),
        ),
        (
            "response_buffer_len=8K",
            HttpConfig::default().with_response_buffer_len(8 * 1024),
        ),
    ];

    for (name, len) in [
        ("static-h2 shape (15.7 KB x 32)", RESPONSE_LEN),
        ("large bodies (4 MB x 32)", LARGE_RESPONSE_LEN),
    ] {
        println!(
            "\n{name}\n{:<26} {:>7} {:>12} {:>13} {:>8} {:>11}",
            "config", "ticks", "data bytes", "bytes/tick", "frames", "max frame"
        );
        for (label, config) in &cases {
            let s = drain_responses(*config, len);
            println!(
                "{:<26} {:>7} {:>12} {:>13} {:>8} {:>11}",
                label,
                s.ticks,
                s.data_bytes,
                s.bytes_per_tick(),
                s.data_frames,
                s.max_frame_payload
            );
        }
    }
}

/// `copy_loops_per_yield` does not bound the send path at all, at any body size, as long as the
/// transport accepts the writes.
///
/// The budget counts rounds of the driver state machine, but pass 2 of
/// [`advance_outbound_sends`][super::super::send] drains each non-incremental stream *to
/// completion* in an inner loop within a single round — so one round already emits everything
/// that flow control allows. `drive` only gives the budget back to the runtime when
/// `poll_flush_outbound` returns `Pending`, which against a real socket means the kernel buffer
/// filled: backpressure is the limiter, not the knob.
///
/// Worth pinning because the knob reads like a throughput/latency tradeoff on the send path and
/// is not one. If this ever fails, the send pump grew a per-round bound and
/// `copy_loops_per_yield` became load-bearing for response throughput.
#[test]
fn yield_budget_does_not_bound_the_send_path() {
    for len in [RESPONSE_LEN, LARGE_RESPONSE_LEN] {
        for loops in [4, 16, 64, 256] {
            let s = drain_responses(HttpConfig::default().with_copy_loops_per_yield(loops), len);
            assert_eq!(
                s.ticks, 1,
                "copy_loops_per_yield={loops} should still drain {STREAMS} x {len}B in one drive \
                 call; got {s:?}",
            );
        }
    }
}

/// `h2_max_frame_size` is `SETTINGS_MAX_FRAME_SIZE`, which is what *we* will accept — it is
/// advertised to the peer and bounds inbound frames. Outbound DATA is bounded by the peer's
/// own advertised maximum. Raising ours must therefore not change the frames we emit; if it
/// does, the send path is reading the wrong side's limit.
#[test]
fn own_max_frame_size_does_not_govern_outbound_framing() {
    let default = drain_responses(HttpConfig::default(), LARGE_RESPONSE_LEN);
    let raised = drain_responses(
        HttpConfig::default().with_h2_max_frame_size(64 * 1024),
        LARGE_RESPONSE_LEN,
    );
    assert_eq!(
        default.max_frame_payload, raised.max_frame_payload,
        "h2_max_frame_size is the inbound limit; raising it changed outbound DATA framing from \
         {default:?} to {raised:?}",
    );
}
