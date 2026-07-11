use crate::{
    Conn, Headers, HttpConfig, HttpContext, Method, Priority, Status,
    h2::{
        H2Driver, H2Error, H2ErrorCode, H2Transport,
        acceptor::{recv::CLIENT_PREFACE, types::DriverState},
        connection::H2Connection,
        frame::{
            FRAME_HEADER_LEN, Frame, FrameHeader, FrameType, continuation as continuation_frame,
            data as data_frame, encode_frame, goaway as goaway_frame, headers as headers_frame,
            rst_stream as rst_stream_frame, settings, window_update as window_update_frame,
        },
        role::Role,
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
pub(super) struct NoopWaker;
impl Wake for NoopWaker {
    fn wake(self: Arc<Self>) {}

    fn wake_by_ref(self: &Arc<Self>) {}
}

pub(super) fn noop_waker() -> Waker {
    Waker::from(Arc::new(NoopWaker))
}

/// A waker that counts wakes — for asserting that a teardown path actually fires a parked
/// task's waker (the recv/send-completion fan-out a stranded handler depends on).
pub(super) struct CountingWaker(pub(super) std::sync::atomic::AtomicUsize);
impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl CountingWaker {
    pub(super) fn count(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A fresh counting waker plus a [`Waker`] backed by it.
pub(super) fn counting_waker() -> (Arc<CountingWaker>, Waker) {
    let counting = Arc::new(CountingWaker(std::sync::atomic::AtomicUsize::new(0)));
    let waker = Waker::from(counting.clone());
    (counting, waker)
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
        Self::new_server_with_config(HttpConfig::default())
    }

    /// Server-role fixture with a custom [`HttpConfig`] — for tests that need a non-default
    /// tuning knob (e.g. a small `h2_max_stream_recv_window_size` to exercise the per-stream
    /// recv buffer cap without flooding a megabyte of DATA).
    pub(super) fn new_server_with_config(config: HttpConfig) -> Self {
        let (driver_transport, peer) = TestTransport::new();
        let context = Arc::new(HttpContext {
            config,
            ..HttpContext::default()
        });
        let connection = H2Connection::new(context);
        let driver = connection.clone().run(driver_transport);
        let peer_hpack = HpackEncoder::new(Arc::new(HeaderObserver::default()), 0, 0, false);
        Self {
            driver,
            connection,
            peer,
            peer_read_cursor: 0,
            peer_hpack,
        }
    }

    /// Construct a client-role fixture. The driver runs the *client* side of the connection
    /// (writes the preface, opens streams with locally-allocated odd ids, reads HEADERS on
    /// its own streams as responses); the test code plays the *server* peer. Pair with
    /// [`Self::complete_handshake_client`], then open streams via
    /// [`H2Connection::open_stream`][crate::h2::H2Connection::open_stream].
    pub(super) fn new_client() -> Self {
        let (driver_transport, peer) = TestTransport::new();
        let connection = H2Connection::new(Arc::new(HttpContext::new()));
        let driver = H2Driver::new(connection.clone(), driver_transport, Role::Client);
        let peer_hpack = HpackEncoder::new(Arc::new(HeaderObserver::default()), 0, 0, false);
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

    /// Open a peer-initiated GET request stream carrying an RFC 9218 `priority` header, for
    /// driving the send pump's priority scheduler. The header value is the canonical
    /// structured-fields form of `priority` (e.g. `u=1, i`).
    pub(super) fn peer_open_stream_with_priority(
        &mut self,
        stream_id: u32,
        path: &str,
        priority: Priority,
        end_stream: bool,
    ) {
        let pseudos = PseudoHeaders::default()
            .with_method(Method::Get)
            .with_path(path)
            .with_scheme("http")
            .with_authority("test");
        let mut headers = Headers::new();
        headers.insert("priority", priority.to_string());
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

    /// Write a peer-side `PRIORITY_UPDATE` frame reprioritizing
    /// `prioritized_stream_id` to `priority`. Carried on the connection control stream
    /// (frame stream id 0); the payload is the 4-byte prioritized stream id plus the
    /// priority's canonical structured-fields value.
    pub(super) fn peer_priority_update(&mut self, prioritized_stream_id: u32, priority: Priority) {
        let mut payload = (prioritized_stream_id & 0x7FFF_FFFF).to_be_bytes().to_vec();
        payload.extend_from_slice(priority.to_string().as_bytes());
        let frame = encode_frame(FrameType::PriorityUpdate, 0, 0, &payload);
        self.peer.write_all(&frame);
    }

    /// Open a peer-initiated request stream with a HEADERS frame that does *not* set
    /// `END_HEADERS` — the header block continues in [`Self::peer_continuation`] frames. For
    /// exercising the inbound HEADERS + CONTINUATION accumulation path (and its flood guard).
    pub(super) fn peer_open_stream_no_end_headers(
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
        headers_frame::encode_prefix(stream_id, end_stream, false, None, block_len, 0, &mut frame)
            .expect("encode HEADERS prefix");
        frame[FRAME_HEADER_LEN..].copy_from_slice(&block);
        self.peer.write_all(&frame);
    }

    /// Open a peer request stream whose HPACK header block is split across a HEADERS frame
    /// (no `END_HEADERS`) followed by a single CONTINUATION frame (`END_HEADERS`), to exercise
    /// inbound block reassembly. The block is encoded whole, then split at `split_at` bytes
    /// (clamped to the block length) — HPACK fragments reassemble byte-wise, so any split is
    /// valid.
    pub(super) fn peer_open_stream_split(
        &mut self,
        stream_id: u32,
        method: Method,
        path: &str,
        end_stream: bool,
        split_at: usize,
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
        let split_at = split_at.min(block.len());

        let head = &block[..split_at];
        let head_len = u32::try_from(head.len()).expect("fragment fits u32");
        let mut frame = vec![0u8; FRAME_HEADER_LEN + head.len()];
        headers_frame::encode_prefix(stream_id, end_stream, false, None, head_len, 0, &mut frame)
            .expect("encode HEADERS prefix");
        frame[FRAME_HEADER_LEN..].copy_from_slice(head);
        self.peer.write_all(&frame);

        self.peer_continuation(stream_id, &block[split_at..], true);
    }

    /// Write a raw CONTINUATION frame carrying `fragment` header-block bytes on `stream_id`,
    /// with the given `END_HEADERS` flag. Bytes are written verbatim, not HPACK-encoded —
    /// tests of the accumulation *bound* pass filler; tests of the happy path pass a real
    /// HPACK fragment.
    pub(super) fn peer_continuation(&mut self, stream_id: u32, fragment: &[u8], end_headers: bool) {
        let len = u32::try_from(fragment.len()).expect("fragment fits u32");
        let mut frame = vec![0u8; continuation_frame::ENCODED_PREFIX_LEN + fragment.len()];
        continuation_frame::encode_prefix(stream_id, end_headers, len, &mut frame)
            .expect("encode CONTINUATION prefix");
        frame[continuation_frame::ENCODED_PREFIX_LEN..].copy_from_slice(fragment);
        self.peer.write_all(&frame);
    }

    /// Client-role fixtures: write a peer-side (server) *response* HEADERS frame on
    /// `stream_id`, carrying a `:status` pseudo-header. `end_stream = true` terminates the
    /// response at this frame (no body); otherwise the caller follows up with DATA / trailers.
    pub(super) fn peer_response_headers(
        &mut self,
        stream_id: u32,
        status: Status,
        end_stream: bool,
    ) {
        let pseudos = PseudoHeaders::default().with_status(status);
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

    /// Escape hatch: write a peer-side HEADERS frame with arbitrary pseudo-headers + fields
    /// and the given `END_STREAM` flag (`END_HEADERS` always set). The valid-shape helpers
    /// (`peer_open_stream` / `peer_trailers` / `peer_response_headers`) bake in well-formed
    /// blocks; this is for the malformed-block paths (e.g. trailers carrying pseudos, or a
    /// trailing HEADERS missing END_STREAM).
    pub(super) fn peer_headers(
        &mut self,
        stream_id: u32,
        pseudos: PseudoHeaders<'static>,
        fields: &Headers,
        end_stream: bool,
    ) {
        let field_section = FieldSection::new(pseudos, fields);
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

    /// Write a peer-side `RST_STREAM` frame on `stream_id` with the given error code.
    pub(super) fn peer_rst_stream(&mut self, stream_id: u32, code: H2ErrorCode) {
        let mut frame = vec![0u8; rst_stream_frame::ENCODED_LEN];
        rst_stream_frame::encode(stream_id, code, &mut frame).expect("encode RST_STREAM");
        self.peer.write_all(&frame);
    }

    /// Write a peer-side connection-level `GOAWAY` frame. `last_stream_id` is carried on
    /// the wire but not consulted by the driver's inbound-GOAWAY path (which just begins a
    /// graceful close regardless), so tests pass whatever reads clearly. No debug data.
    pub(super) fn peer_goaway(&mut self, last_stream_id: u32, code: H2ErrorCode) {
        let mut frame = vec![0u8; goaway_frame::encoded_len(0)];
        goaway_frame::encode(last_stream_id, code, &[], &mut frame).expect("encode GOAWAY");
        self.peer.write_all(&frame);
    }

    /// Write a peer-side `WINDOW_UPDATE` frame. `stream_id = 0` credits the connection-level
    /// send window; a non-zero id credits that stream's send window.
    pub(super) fn peer_window_update(&mut self, stream_id: u32, increment: u32) {
        let mut frame = vec![0u8; window_update_frame::ENCODED_LEN];
        window_update_frame::encode(stream_id, increment, &mut frame)
            .expect("encode WINDOW_UPDATE");
        self.peer.write_all(&frame);
    }

    /// Drive the connection through the standard server-role handshake: client preface
    /// in, initial SETTINGS + connection-level WINDOW_UPDATE out, peer SETTINGS in,
    /// SETTINGS_ACK out. Asserts the driver lands in `Running` and that the expected
    /// frames appeared on the wire.
    pub(super) fn complete_handshake(&mut self) {
        self.complete_handshake_with_peer_settings(H2Settings::default());
    }

    /// Like [`Self::complete_handshake`] but the peer's SETTINGS frame carries `settings` —
    /// e.g. a small `initial_window_size` to seed a tight per-stream *send* window for
    /// flow-control tests (a stream opened after this seeds its send window from the peer's
    /// effective initial window size).
    pub(super) fn complete_handshake_with_peer_settings(&mut self, settings: H2Settings) {
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

        // Peer SETTINGS so the driver has something to ACK and the recv pump has parsed at
        // least one peer frame — keeps the post-handshake start point realistic.
        let mut buf = vec![0u8; settings::encoded_len(&settings)];
        settings::encode(&settings, &mut buf).expect("encode settings");
        self.peer.write_all(&buf);
        let _ = self.tick();

        // Burn off handshake bytes so subsequent assertions see only test-relevant frames.
        let _ = self.next_outbound_bytes();
    }

    /// Client-role handshake: the client driver writes its preface + initial SETTINGS
    /// (+ WINDOW_UPDATE) and reaches `Running` without reading anything first; then the
    /// server peer sends its SETTINGS, which the client applies and ACKs. Burns the
    /// handshake outbound so subsequent assertions see only test-relevant frames.
    pub(super) fn complete_handshake_client(&mut self) {
        let _ = self.tick();
        if self.driver.state != DriverState::Running {
            let _ = self.tick();
        }
        assert_eq!(
            self.driver.state,
            DriverState::Running,
            "client should reach Running after writing its preface + SETTINGS",
        );

        let settings = H2Settings::default();
        let mut buf = vec![0u8; settings::encoded_len(&settings)];
        settings::encode(&settings, &mut buf).expect("encode settings");
        self.peer.write_all(&buf);
        let _ = self.tick();

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
pub(super) fn decode_frames(bytes: &[u8]) -> Vec<Frame> {
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
pub(super) fn count_goaways(frames: &[Frame]) -> usize {
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
