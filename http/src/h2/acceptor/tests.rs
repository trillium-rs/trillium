//! Wire-level tests for [`H2Driver`].
//!
//! These tests sit above [`StreamState`][super::super::transport::StreamState] — they
//! drive the public(-ish) acceptor surface and assert what bytes appear on the wire, not
//! which per-stream booleans flipped. The bag-of-atomics / lifecycle-enum refactor is
//! below this layer; a future reader who only sees the test diff should not be able to
//! tell which implementation is in effect.
//!
//! See [`h2-lifecycle-refactor-plan`] (memory) for the enumerated tests this module is
//! meant to grow.

use crate::{
    Body, Conn, Headers, HttpContext, Method, Status,
    h2::{
        H2Driver, H2Error, H2ErrorCode, H2Transport,
        acceptor::{
            recv::CLIENT_PREFACE,
            types::{CloseOutcome, DriverState},
        },
        connection::H2Connection,
        frame::{
            FRAME_HEADER_LEN, Frame, FrameHeader, data as data_frame, headers as headers_frame,
            settings,
        },
        settings::H2Settings,
    },
    headers::{
        header_observer::HeaderObserver,
        hpack::{FieldSection, HpackEncoder, PseudoHeaders},
    },
};
use std::{
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};
use trillium_testing::TestTransport;

/// Marker waker — the driver's `drive` calls `wake_by_ref` to ensure the executor
/// re-polls after the cooperative-yield bound. Tests poll synchronously, so we don't
/// need a real wake; we just observe whether a poll returned `Ready` or `Pending`.
struct NoopWaker;
impl Wake for NoopWaker {
    fn wake(self: Arc<Self>) {}

    fn wake_by_ref(self: &Arc<Self>) {}
}

fn noop_waker() -> Waker {
    Waker::from(Arc::new(NoopWaker))
}

/// Paired-transport `H2Driver` test fixture. The driver runs over one half of a
/// `TestTransport` pair; the test code drives "the peer" through the other half — writing
/// frames synchronously into the driver's read side and pulling outbound bytes from the
/// driver's write side.
///
/// Each `tick` advances the driver one `drive` call (one full `copy_loops_per_yield`
/// budget). Outbound bytes are revealed incrementally via [`Self::next_outbound_bytes`]
/// so multi-step tests can isolate what each step emitted.
pub(super) struct DriverFixture {
    pub(super) driver: H2Driver<TestTransport>,
    pub(super) connection: Arc<H2Connection>,
    pub(super) peer: TestTransport,
    peer_read_cursor: usize,

    /// Peer-side HPACK encoder. Independent dynamic table from the driver's encoder so
    /// frames the test writes peer-to-driver are encoded against this state, while frames
    /// the driver writes back are encoded against its own. Configured with
    /// `local_preferred_size = 0` so every header line is emitted as a literal-
    /// without-indexing — the driver's decoder learns nothing from the lines, keeping the
    /// two dynamic tables in (trivial) sync without bookkeeping.
    peer_hpack: HpackEncoder,
}

impl DriverFixture {
    /// Construct a server-role fixture. The driver starts in `AwaitingPreface`; tests
    /// that need the steady state should follow up with [`Self::complete_handshake`].
    pub(super) fn new_server() -> Self {
        let (driver_transport, peer) = TestTransport::new();
        let context = Arc::new(HttpContext::new());
        let connection = H2Connection::new(context);
        let driver = connection.clone().run(driver_transport);
        let peer_hpack = HpackEncoder::new(Arc::new(HeaderObserver::default()), 0, 0);
        Self {
            driver,
            connection,
            peer,
            peer_read_cursor: 0,
            peer_hpack,
        }
    }

    /// Open a peer-initiated request stream by writing a HEADERS frame with the supplied
    /// pseudo-headers. Body-less requests (`end_stream = true`) terminate the stream's
    /// recv side at this frame; otherwise the caller is responsible for sending a
    /// terminating DATA frame with `end_stream = true` (or RST_STREAM) to complete it.
    ///
    /// HEADERS are framed with `end_headers = true` (no CONTINUATION continuation).
    pub(super) fn peer_open_stream(
        &mut self,
        stream_id: u32,
        method: Method,
        path: &str,
        end_stream: bool,
    ) {
        let pseudos = PseudoHeaders::default()
            .with_method(method)
            .with_path(path)
            .with_scheme("http")
            .with_authority("test");
        let headers = Headers::new();
        let field_section = FieldSection::new(pseudos, &headers);
        let mut block = Vec::new();
        self.peer_hpack.encode(&field_section, &mut block);

        let block_len = u32::try_from(block.len()).expect("block fits u32");
        let mut frame = vec![0u8; FRAME_HEADER_LEN + block.len()];
        headers_frame::encode_prefix(stream_id, end_stream, true, None, block_len, 0, &mut frame)
            .expect("encode HEADERS prefix");
        frame[FRAME_HEADER_LEN..].copy_from_slice(&block);
        self.peer.write_all(&frame);
    }

    /// Write a peer-side trailing HEADERS frame on an existing `stream_id`. RFC 9113 §8.1
    /// requires `END_STREAM` and no pseudo-headers on the trailer block; both invariants
    /// are baked in here so tests just supply the trailer fields.
    pub(super) fn peer_trailers(&mut self, stream_id: u32, trailers: &Headers) {
        let field_section = FieldSection::new(PseudoHeaders::default(), trailers);
        let mut block = Vec::new();
        self.peer_hpack.encode(&field_section, &mut block);
        let block_len = u32::try_from(block.len()).expect("block fits u32");

        let mut frame = vec![0u8; FRAME_HEADER_LEN + block.len()];
        headers_frame::encode_prefix(stream_id, true, true, None, block_len, 0, &mut frame)
            .expect("encode HEADERS prefix");
        frame[FRAME_HEADER_LEN..].copy_from_slice(&block);
        self.peer.write_all(&frame);
    }

    /// Write a peer-side DATA frame carrying `payload` bytes on `stream_id`, with the
    /// supplied `end_stream` flag. No padding.
    pub(super) fn peer_data(&mut self, stream_id: u32, payload: &[u8], end_stream: bool) {
        let payload_len = u32::try_from(payload.len()).expect("data fits u32");
        let mut frame = vec![0u8; FRAME_HEADER_LEN + payload.len()];
        data_frame::encode_prefix(stream_id, end_stream, payload_len, 0, &mut frame)
            .expect("encode DATA prefix");
        frame[FRAME_HEADER_LEN..].copy_from_slice(payload);
        self.peer.write_all(&frame);
    }

    /// Drive the connection through the standard server-role handshake: client preface
    /// in, initial SETTINGS + connection-level WINDOW_UPDATE out, peer SETTINGS in,
    /// SETTINGS_ACK out. Asserts the driver lands in `Running` and that the expected
    /// frames appeared on the wire.
    pub(super) fn complete_handshake(&mut self) {
        // Peer writes the 24-byte preface immediately; a real client would as well.
        self.peer.write_all(CLIENT_PREFACE);

        // Drive through preface read → server SETTINGS queue → running. One tick is
        // usually sufficient (drive's inner copy_loops_per_yield budget covers it),
        // but tick a second time defensively in case scheduling shifts.
        let _ = self.tick();
        if self.driver.state != DriverState::Running {
            let _ = self.tick();
        }
        assert_eq!(
            self.driver.state,
            DriverState::Running,
            "driver should reach Running after preface",
        );

        // Peer writes an empty SETTINGS so the driver has something to ACK and the
        // recv pump has parsed at least one peer frame — keeps the post-handshake
        // start point realistic.
        let empty_settings = H2Settings::default();
        let mut buf = vec![0u8; settings::encoded_len(&empty_settings)];
        settings::encode(&empty_settings, &mut buf).expect("encode settings");
        self.peer.write_all(&buf);
        let _ = self.tick();

        // Burn off handshake bytes so subsequent assertions see only test-relevant frames.
        let _ = self.next_outbound_bytes();
    }

    /// One poll of the driver's `drive`. Returns `Ready(item)` if the driver yielded
    /// (new Conn or terminal result); `Pending` otherwise. Internally `drive` consumes
    /// up to `copy_loops_per_yield` of its inner work units per call.
    pub(super) fn tick(&mut self) -> Poll<Option<Result<Conn<H2Transport>, H2Error>>> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        self.driver.drive(&mut cx)
    }

    /// Bytes the driver has written to the wire since the last call to this method (or
    /// since construction). Empty if no new outbound bytes have been flushed.
    pub(super) fn next_outbound_bytes(&mut self) -> Vec<u8> {
        let all = self.peer.snapshot();
        if all.len() <= self.peer_read_cursor {
            return Vec::new();
        }
        let bytes = all[self.peer_read_cursor..].to_vec();
        self.peer_read_cursor = all.len();
        bytes
    }

    /// Drain the next outbound bytes and decode them into a flat list of frames. Panics
    /// if the buffer doesn't end on a frame boundary or if any frame fails to decode —
    /// the wire-format invariants the driver upholds should be unconditional.
    pub(super) fn next_outbound_frames(&mut self) -> Vec<Frame> {
        decode_frames(&self.next_outbound_bytes())
    }
}

/// Decode a sequence of complete h2 frames from `bytes`. Panics on incomplete or
/// malformed input — the caller is expected to pass a buffer the driver has flushed in
/// full, so partial frames are a fixture bug rather than something to recover from.
fn decode_frames(bytes: &[u8]) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let header = FrameHeader::decode(&bytes[offset..]).expect("incomplete frame header");
        let frame_len = FRAME_HEADER_LEN + header.length as usize;
        let frame_bytes = &bytes[offset..offset + frame_len];
        let (frame, _consumed) = Frame::decode(frame_bytes).expect("frame decode");
        frames.push(frame);
        offset += frame_len;
    }
    frames
}

/// Convenience predicate — fixture parsing surfaces every frame as a `Frame` enum, and
/// most assertions count occurrences by variant rather than caring about fields.
fn count_goaways(frames: &[Frame]) -> usize {
    frames
        .iter()
        .filter(|f| matches!(f, Frame::Goaway { .. }))
        .count()
}

/// Fixture sanity check — the standard server-role handshake should produce a SETTINGS
/// frame and an initial connection-level WINDOW_UPDATE on the wire. Validates the test
/// helper machinery before relying on it in the lifecycle tests below.
#[test]
fn fixture_handshake_emits_settings_and_window_update() {
    let mut fx = DriverFixture::new_server();
    fx.peer.write_all(CLIENT_PREFACE);
    let _ = fx.tick();
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    let settings_count = frames
        .iter()
        .filter(|f| matches!(f, Frame::Settings(_)))
        .count();
    let wu_count = frames
        .iter()
        .filter(|f| matches!(f, Frame::WindowUpdate { .. }))
        .count();
    assert!(
        settings_count >= 1,
        "expected initial SETTINGS in handshake outbound, got frames: {frames:?}",
    );
    assert!(
        wu_count >= 1,
        "expected initial WINDOW_UPDATE in handshake outbound, got frames: {frames:?}",
    );
}

/// Driver yields a `Conn` for a well-formed peer HEADERS opening a new stream. Validates
/// `peer_open_stream` + the recv-pump → `Action::Emit` path end-to-end before lifecycle
/// tests rely on it for setup.
#[test]
fn peer_headers_opening_stream_yields_conn() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let polled = fx.tick();
    match polled {
        Poll::Ready(Some(Ok(conn))) => {
            assert_eq!(conn.method(), Method::Get);
            assert_eq!(conn.path(), "/");
        }
        other => panic!("expected Ready(Some(Ok(conn))) yielding the new request, got {other:?}"),
    }
}

/// Closing → Drained is gated on the in-flight stream predicate: while any stream has an
/// active send cursor or unfinished recv side, the driver stays in Closing — only once
/// both clear can it transition to Drained. Validates the behavior the wip-commit
/// docstring promises:
///
/// > Defer the transition while in-flight streams still have outbound (SendCursor not yet
/// > Complete) OR inbound (`recv.eof` not yet set) work.
///
/// Wire-level assertion: after begin_close with an in-flight stream open, no FIN-style
/// close happens (state stays Closing); after peer ends the stream's recv side, the
/// transition fires.
#[test]
fn closing_to_drained_waits_for_in_flight_stream() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open stream 1 with `end_stream=false` so the recv side stays in-flight after the
    // request HEADERS — `has_pending_recv` will be true until peer END_STREAM lands.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    // Hold the conn for the duration of the test — dropping it would tear down the
    // H2Transport and let the stream complete via a different path than the one we're
    // exercising here.
    let _conn_guard = conn;

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "in-flight stream's open recv side should hold the driver in Closing",
    );

    // Peer closes its half of stream 1. Driver's recv pump (still running in Closing per
    // the wip commit) picks up END_STREAM, recv.eof flips, predicate clears, transition
    // fires.
    fx.peer_data(1, &[], true);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "with the last in-flight stream's recv side closed, Closing should advance to Drained",
    );
}

/// Trailers staged via `submit_trailers` while a `SendCursor` is parked in `Body` phase
/// (waiting on the upgrade outbound buffer to fill) must still reach the wire as a
/// trailing HEADERS frame on the next driver tick. This is the trailers-stranding
/// regression that motivated the recent `transition_to_trailers` fallback in
/// [`send`][super::send]: previously, by the time the cursor reached `Body` EOF, the
/// only pickup site for `pending_trailers` had already run, and the trailers were lost.
#[test]
fn submit_trailers_lands_on_wire_after_body_parked() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // submit_upgrade installs an `H2OutboundReader` as the body, signals submission
    // completion at END_HEADERS, and leaves the cursor parked in Body until either bytes
    // appear in the outbound queue or `outbound_close_requested` flips.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_upgrade(1, pseudos, Headers::new());

    // Tick: HEADERS go out, cursor parks in Body (empty outbound, close not requested).
    let _ = fx.tick();
    let headers_round = fx.next_outbound_frames();
    assert!(
        headers_round.iter().any(|f| matches!(
            f,
            Frame::Headers {
                stream_id: 1,
                end_stream: false,
                ..
            }
        )),
        "response HEADERS (without END_STREAM) should be on the wire after first tick; got \
         {headers_round:?}",
    );

    // Outside the driver task: stage trailers + request close. The driver's send pump
    // must pick this up on its next tick despite the cursor being parked.
    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.connection
        .submit_trailers(1, trailers)
        .expect("submit_trailers on a live stream");

    let _ = fx.tick();
    let trailing = fx.next_outbound_frames();
    let trailing_headers = trailing
        .iter()
        .filter(|f| {
            matches!(
                f,
                Frame::Headers {
                    stream_id: 1,
                    end_stream: true,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        trailing_headers, 1,
        "exactly one trailing HEADERS with END_STREAM should land on the wire after \
         submit_trailers; got {trailing:?}",
    );
}

/// An extended-CONNECT upgrade stream sitting at `UpgradeOpen` with an empty outbound
/// queue (handler hasn't written, peer hasn't sent more) must let the driver park —
/// returning `Poll::Pending` *without* self-waking. The `SendCursor` is parked in `Body`
/// because the upgrade body's `poll_read` returned `Pending` (it registered the outbound
/// waker), so there's no progress to make until an external wake arrives.
///
/// Regression: `has_pending_outbound_progress` used to report `true` for any `Body`-phase
/// cursor with a positive send window, ignoring that the body had parked. That defeated
/// `park`, so the driver burned through `copy_loops_per_yield` every poll and re-armed via
/// the cooperative-yield `wake_by_ref` — a busy-spin emitting hundreds of thousands of
/// `drive` log lines instead of sleeping. Asserting the waker isn't fired pins the park.
#[test]
fn idle_upgrade_open_stream_parks_without_self_waking() {
    /// Wake counter so we can tell a clean park (no wake) from a self-wake spin.
    struct CountingWaker(std::sync::atomic::AtomicUsize);
    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Drive into the parked-upgrade state: HEADERS go out, the cursor parks in Body with an
    // empty outbound queue and no close requested.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_upgrade(1, pseudos, Headers::new());
    let _ = fx.tick();
    let _ = fx.next_outbound_bytes();

    // The next poll has no work: no inbound frame, no outbound bytes, body parked. The
    // driver must register on its wakers and return Pending without re-arming itself.
    let counter = Arc::new(CountingWaker(std::sync::atomic::AtomicUsize::new(0)));
    let waker = Waker::from(counter.clone());
    let mut cx = Context::from_waker(&waker);
    let polled = fx.driver.drive(&mut cx);
    assert!(
        matches!(polled, Poll::Pending),
        "idle upgrade-open driver should park, got {polled:?}",
    );
    assert_eq!(
        counter.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "driver self-woke instead of parking — busy-spin on an idle UpgradeOpen stream",
    );
}

/// A server that finishes responding (trailing HEADERS + END_STREAM) while the peer's
/// request half is still open is only at half-closed (local), not closed (RFC 9113
/// §5.1). The peer's subsequent END_STREAM — a zero-length DATA frame closing its
/// request half — is legal and must complete the stream cleanly. The bug this pins:
/// server-role teardown removes the stream on send completion regardless of recv state,
/// so the peer's END_STREAM lands on a stream the driver has already forgotten and is
/// answered with a spurious `RST_STREAM(STREAM_CLOSED)`. That RST races back to the peer
/// and destroys the just-delivered trailers — the gRPC "stream ended without grpc-status
/// trailer" failure under load.
#[test]
fn peer_end_stream_after_server_trailers_is_not_reset() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open the request stream WITHOUT END_STREAM — the peer's request half stays open,
    // exactly as a gRPC client's upgrade-style request stream does before it has sent its
    // own terminator.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Server responds via the upgrade path and stages trailers, completing its send half
    // while the peer's request half is still open.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_upgrade(1, pseudos, Headers::new());
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.connection
        .submit_trailers(1, trailers)
        .expect("submit_trailers on a live stream");
    let _ = fx.tick();
    let trailing = fx.next_outbound_frames();
    assert!(
        trailing.iter().any(|f| matches!(
            f,
            Frame::Headers {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "server's trailing HEADERS with END_STREAM should be on the wire; got {trailing:?}",
    );

    // Now the peer closes its request half: a zero-length DATA frame with END_STREAM.
    // This arrives strictly after the server's trailers — the deterministic version of
    // the load-dependent race.
    fx.peer_data(1, &[], true);
    let _ = fx.tick();

    let after = fx.next_outbound_frames();
    assert!(
        !after
            .iter()
            .any(|f| matches!(f, Frame::RstStream { stream_id: 1, .. })),
        "peer's END_STREAM on a half-closed-local stream must close cleanly, not earn a \
         RST_STREAM; got {after:?}",
    );
}

/// The send pump runs in `Closing` (not just `Running`): once we've begun closing, any
/// stream with a staged submission must still be framed and put on the wire — gRPC and
/// other late-trailer patterns submit the response right around the same time the
/// shutdown decision fires, and dropping the in-flight response would be a regression.
/// The wip-commit changed the send-pump's run condition from `Running` to
/// `Running | Closing` for this reason; this test pins it.
#[test]
fn send_pump_emits_response_in_closing() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Stage a small response submission, then immediately begin_close — the send pump
    // hasn't picked it up yet, so the question is whether the pump runs in Closing.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let body = Body::new_static(b"hi" as &[u8]);
    let _submit = fx
        .connection
        .submit_send(1, pseudos, Headers::new(), Some(body));
    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    let response_headers = frames
        .iter()
        .filter(|f| matches!(f, Frame::Headers { stream_id: 1, .. }))
        .count();
    let data_frames = frames
        .iter()
        .filter(|f| matches!(f, Frame::Data { stream_id: 1, .. }))
        .count();
    assert!(
        response_headers >= 1,
        "send pump should emit response HEADERS for stream 1 while Closing; got {frames:?}",
    );
    assert!(
        data_frames >= 1,
        "send pump should emit DATA for stream 1 while Closing; got {frames:?}",
    );
    let end_stream_data = frames.iter().any(|f| {
        matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )
    });
    assert!(
        end_stream_data,
        "send pump should terminate stream 1 with END_STREAM; got {frames:?}",
    );
}

/// The recv pump runs in `Closing` (not just `Running`): trailing HEADERS the peer sends
/// after the driver has begun closing must still be decoded and stashed on the in-flight
/// stream's `recv.trailers` slot — otherwise gRPC trailers can vanish under shutdown
/// pressure. The wip-commit changed the read-side pump's run condition from
/// `Running` to `Running | Closing` for precisely this reason; this test pins the
/// behavior so the lifecycle refactor preserves it.
#[test]
fn recv_pump_decodes_trailing_headers_in_closing() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // POST with end_stream=false leaves the request body open — we'll send trailing
    // HEADERS as the terminator instead of DATA(END_STREAM).
    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    let state = fx
        .connection
        .streams_lock()
        .get(&1)
        .cloned()
        .expect("stream 1 registered");

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(fx.driver.state, DriverState::Closing);

    // Trailing HEADERS arrive *after* our GOAWAY went out. The recv-pump-in-Closing rule
    // says we keep decoding for streams already in flight.
    let mut trailers_in = Headers::new();
    trailers_in.insert("grpc-status", "0");
    trailers_in.insert("grpc-message", "ok");
    fx.peer_trailers(1, &trailers_in);
    let _ = fx.tick();

    let stashed = state
        .recv
        .trailers
        .lock()
        .expect("recv.trailers mutex poisoned")
        .clone()
        .expect("driver should have stashed trailers from the post-GOAWAY frame");
    assert_eq!(stashed.get_str("grpc-status"), Some("0"));
    assert_eq!(stashed.get_str("grpc-message"), Some("ok"));
}

/// A peer HEADERS opening a *new* stream while the driver is in `Closing` must not be
/// yielded as a `Conn` — once we've sent GOAWAY, the peer shouldn't be opening new
/// streams, and even if it does we mustn't dispatch a handler for one we're about to tear
/// down. Pairs with [`closing_to_drained_waits_for_in_flight_stream`] above (which keeps
/// the driver in Closing long enough to observe this branch).
#[test]
fn closing_discards_new_stream_headers() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Keep the driver in Closing by holding an in-flight stream with an open recv side.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let stream_one = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(fx.driver.state, DriverState::Closing);
    let _ = fx.next_outbound_bytes();

    // Peer (misbehaving) opens a new stream past the GOAWAY.
    fx.peer_open_stream(3, Method::Get, "/late", true);
    let polled = fx.tick();
    assert!(
        !matches!(polled, Poll::Ready(Some(Ok(_)))),
        "post-GOAWAY HEADERS opening a new stream must not yield a Conn; got {polled:?}",
    );

    // Cleanup: drop the held stream-1 conn so its Drop doesn't outlive the fixture and
    // accidentally interleave assertions in a later test (unimportant for correctness;
    // makes the test scope explicit).
    drop(stream_one);
}

/// `begin_close` is idempotent: a second call once the driver is already `Closing` (or
/// `Drained`) does not queue another GOAWAY and does not overwrite the prior close
/// outcome. The peer-mirror case in the wild — peer GOAWAY arrives after we've already
/// begun closing — would otherwise ping-pong, each side re-arming on the other's frame.
///
/// Asserts at the wire level (count of GOAWAY frames in outbound bytes) so the
/// future lifecycle-enum refactor doesn't change what this test exercises.
#[test]
fn begin_close_is_idempotent() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    assert_eq!(fx.driver.state, DriverState::Running);

    // First close: graceful. Drains outbound to put the GOAWAY on the wire.
    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(fx.driver.state, DriverState::Drained);
    let first_round = fx.next_outbound_frames();
    assert_eq!(
        count_goaways(&first_round),
        1,
        "graceful begin_close should emit exactly one GOAWAY; got {first_round:?}",
    );
    let first_goaway_code = first_round.iter().find_map(|f| match f {
        Frame::Goaway { error_code, .. } => Some(*error_code),
        _ => None,
    });
    assert_eq!(
        first_goaway_code,
        Some(H2ErrorCode::NoError),
        "graceful close should queue NoError, got {first_goaway_code:?}",
    );

    // Second close: protocol error. Must be a no-op — no fresh GOAWAY, state unchanged.
    fx.driver
        .begin_close(CloseOutcome::Protocol(H2ErrorCode::InternalError));
    let _ = fx.tick();
    let second_round = fx.next_outbound_frames();
    assert_eq!(
        count_goaways(&second_round),
        0,
        "second begin_close after Closing/Drained must not re-queue GOAWAY; got {second_round:?}",
    );
}
