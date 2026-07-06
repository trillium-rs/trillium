use super::{Body, BodyFraming, BufWriter, hex_width};
use crate::{HttpConfig, body::BodySource, headers::Headers};
use futures_lite::{AsyncRead, AsyncWriteExt, future::block_on};
use pretty_assertions::assert_eq;
use std::{
    collections::VecDeque,
    io::Result,
    pin::Pin,
    task::{Context, Poll},
};

enum Step {
    Data(&'static [u8]),
    Pending,
    Eof,
}

/// Scripted body source: each `poll_read` performs the next step. `Data` longer than the
/// provided buffer fills it, and the remainder becomes the next step.
struct ScriptSource {
    steps: VecDeque<Step>,
    trailers: Option<Headers>,
}

impl ScriptSource {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            trailers: None,
        }
    }

    fn with_trailers(mut self, trailers: Headers) -> Self {
        self.trailers = Some(trailers);
        self
    }
}

impl AsyncRead for ScriptSource {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        match self.steps.pop_front() {
            Some(Step::Data(data)) => {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                if n < data.len() {
                    self.steps.push_front(Step::Data(&data[n..]));
                }
                Poll::Ready(Ok(n))
            }
            Some(Step::Pending) => Poll::Pending,
            Some(Step::Eof) | None => Poll::Ready(Ok(0)),
        }
    }
}

impl BodySource for ScriptSource {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        self.get_mut().trailers.take()
    }
}

fn write_wire(body: Body, framing: BodyFraming, config: &HttpConfig) -> (Vec<u8>, Option<Headers>) {
    block_on(async {
        let mut out = Vec::new();
        let mut sink = BufWriter::new_with_buffer(Vec::with_capacity(512), &mut out, 2048);
        let trailers = body.write_into(&mut sink, framing, config).await.unwrap();
        sink.flush().await.unwrap();
        drop(sink);
        (out, trailers)
    })
}

/// Drive the legacy framed-`AsyncRead` path the way `Conn::send` used to: copy the body
/// into the sink, no trailer handling (callers stitched that separately in both worlds).
fn legacy_wire(mut body: Body) -> (Vec<u8>, Option<Headers>) {
    block_on(async {
        let mut out = Vec::new();
        let mut sink = BufWriter::new_with_buffer(Vec::with_capacity(512), &mut out, 2048);
        crate::copy(&mut body, &mut sink, 16).await.unwrap();
        sink.flush().await.unwrap();
        drop(sink);
        (out, body.trailers())
    })
}

const CONFIG: &HttpConfig = &HttpConfig::DEFAULT;

#[test]
fn empty_body_by_framing() {
    assert_eq!(write_wire(Body::default(), BodyFraming::Raw, CONFIG).0, b"");
    assert_eq!(
        write_wire(Body::default(), BodyFraming::H3Data, CONFIG).0,
        b""
    );
    assert_eq!(
        write_wire(
            Body::default(),
            BodyFraming::Chunked { keep_open: true },
            CONFIG
        )
        .0,
        b""
    );
    // unlike the legacy AsyncRead impl, an empty chunked body still terminates the stream
    assert_eq!(
        write_wire(
            Body::default(),
            BodyFraming::Chunked { keep_open: false },
            CONFIG
        )
        .0,
        b"0\r\n"
    );
}

#[test]
fn static_raw() {
    let (wire, trailers) = write_wire(Body::from("hello world"), BodyFraming::Raw, CONFIG);
    assert_eq!(wire, b"hello world");
    assert!(trailers.is_none());
}

#[test]
fn static_raw_larger_than_sink_capacity() {
    let content = "x".repeat(10_000);
    let (wire, _) = write_wire(
        Body::new_static(content.clone().into_bytes()),
        BodyFraming::Raw,
        CONFIG,
    );
    assert_eq!(wire, content.as_bytes());
}

#[test]
fn static_chunked() {
    let (wire, _) = write_wire(
        Body::from("hello world"),
        BodyFraming::Chunked { keep_open: false },
        CONFIG,
    );
    assert_eq!(wire, b"B\r\nhello world\r\n0\r\n");
}

#[test]
fn static_chunked_keep_open() {
    let (wire, _) = write_wire(
        Body::from("hello world"),
        BodyFraming::Chunked { keep_open: true },
        CONFIG,
    );
    assert_eq!(wire, b"B\r\nhello world\r\n");
}

#[test]
fn static_h3() {
    let (wire, _) = write_wire(Body::from("hello world"), BodyFraming::H3Data, CONFIG);
    // DATA frame: type 0x00, one-byte varint length 11, payload
    assert_eq!(wire, b"\x00\x0bhello world");
}

#[test]
fn streaming_unknown_length_chunked() {
    let body = Body::new_streaming(
        ScriptSource::new([Step::Data(b"hello"), Step::Data(b"wor"), Step::Eof]),
        None,
    );
    let (wire, _) = write_wire(body, BodyFraming::Chunked { keep_open: false }, CONFIG);
    assert_eq!(wire, b"5\r\nhello\r\n3\r\nwor\r\n0\r\n");
}

#[test]
fn streaming_chunked_keep_open_omits_terminator() {
    let body = Body::new_streaming(ScriptSource::new([Step::Data(b"hello"), Step::Eof]), None);
    let (wire, _) = write_wire(body, BodyFraming::Chunked { keep_open: true }, CONFIG);
    assert_eq!(wire, b"5\r\nhello\r\n");
}

#[test]
fn streaming_chunked_prefix_slide() {
    // 300-byte read: prefix `12C\r\n` is narrower than the width reserved for an
    // 8 KiB-cap chunk, exercising the payload slide.
    let content = "y".repeat(300);
    let content: &'static [u8] = content.into_bytes().leak();
    let body = Body::new_streaming(ScriptSource::new([Step::Data(content), Step::Eof]), None);
    let (wire, _) = write_wire(body, BodyFraming::Chunked { keep_open: false }, CONFIG);
    let mut expected = b"12C\r\n".to_vec();
    expected.extend_from_slice(content);
    expected.extend_from_slice(b"\r\n0\r\n");
    assert_eq!(wire, expected);
}

#[test]
fn streaming_known_length_raw_stops_at_len() {
    // The source would offer more, but the declared length caps what is read and sent.
    let body = Body::new_streaming(
        ScriptSource::new([Step::Data(b"hello"), Step::Data(b"world"), Step::Eof]),
        Some(5),
    );
    let (wire, _) = write_wire(body, BodyFraming::Raw, CONFIG);
    assert_eq!(wire, b"hello");
}

#[test]
fn streaming_known_length_h3_single_frame() {
    let body = Body::new_streaming(
        ScriptSource::new([Step::Data(b"hello"), Step::Data(b" world"), Step::Eof]),
        Some(11),
    );
    let (wire, _) = write_wire(body, BodyFraming::H3Data, CONFIG);
    // one DATA frame header spans both reads
    assert_eq!(wire, b"\x00\x0bhello world");
}

#[test]
fn streaming_unknown_length_h3_frame_per_read() {
    let body = Body::new_streaming(
        ScriptSource::new([Step::Data(b"hello"), Step::Data(b" world"), Step::Eof]),
        None,
    );
    let (wire, _) = write_wire(body, BodyFraming::H3Data, CONFIG);
    // reads are under 64 bytes, so each length varint is one byte (reserved width was
    // two), exercising the payload slide
    assert_eq!(wire, b"\x00\x05hello\x00\x06 world");
}

#[test]
fn trailers_are_returned_after_eof() {
    let mut trailers = Headers::new();
    trailers.insert("x-checksum", "abc123");
    let body = Body::new_with_trailers(
        ScriptSource::new([Step::Data(b"hello"), Step::Eof]).with_trailers(trailers),
        None,
    );
    let (wire, returned) = write_wire(body, BodyFraming::Chunked { keep_open: false }, CONFIG);
    assert_eq!(wire, b"5\r\nhello\r\n0\r\n");
    assert_eq!(
        returned.unwrap().get_str("x-checksum"),
        Some("abc123"),
        "trailers should be returned after the source reaches eof"
    );
}

#[test]
fn chunk_len_floor_still_makes_progress() {
    let config = HttpConfig::default().with_body_write_chunk_len(0);
    let content = "z".repeat(100);
    let content: &'static [u8] = content.into_bytes().leak();
    let body = Body::new_streaming(ScriptSource::new([Step::Data(content), Step::Eof]), None);
    let (wire, _) = write_wire(body, BodyFraming::Chunked { keep_open: false }, &config);
    // clamped chunk size splits the content but the decoded payload is intact
    assert_eq!(decode_chunked(&wire), content);
}

#[test]
fn small_chunk_len_produces_multiple_h3_frames() {
    let config = HttpConfig::default().with_body_write_chunk_len(16);
    let content = "w".repeat(40);
    let content: &'static [u8] = content.into_bytes().leak();
    let body = Body::new_streaming(ScriptSource::new([Step::Data(content), Step::Eof]), None);
    let (wire, _) = write_wire(body, BodyFraming::H3Data, &config);
    assert_eq!(
        wire,
        b"\x00\x10wwwwwwwwwwwwwwww\x00\x10wwwwwwwwwwwwwwww\x00\x08wwwwwwww"
    );
}

/// Minimal chunked-transfer decoder for asserting payload integrity when exact chunk
/// boundaries aren't the point of the test.
fn decode_chunked(mut wire: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = wire.windows(2).position(|w| w == b"\r\n").unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&wire[..line_end]).unwrap(), 16).unwrap();
        wire = &wire[line_end + 2..];
        if size == 0 {
            assert!(
                wire.is_empty(),
                "expected nothing after the last-chunk marker"
            );
            return out;
        }
        out.extend_from_slice(&wire[..size]);
        assert_eq!(&wire[size..size + 2], b"\r\n");
        wire = &wire[size + 2..];
    }
}

mod differential {
    //! The legacy framed `AsyncRead` impl on `Body` is frozen but retained; these tests pin
    //! `write_into` to its wire output for sources whose read sizes match in both worlds.
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn static_raw_matches_legacy() {
        let (legacy, _) = legacy_wire(Body::from("some static content"));
        let (wire, _) = write_wire(Body::from("some static content"), BodyFraming::Raw, CONFIG);
        assert_eq!(legacy, wire);
    }

    #[test]
    fn streaming_known_length_raw_matches_legacy() {
        let steps = || [Step::Data(b"hello"), Step::Data(b" world"), Step::Eof];
        let (legacy, _) = legacy_wire(Body::new_streaming(ScriptSource::new(steps()), Some(11)));
        let (wire, _) = write_wire(
            Body::new_streaming(ScriptSource::new(steps()), Some(11)),
            BodyFraming::Raw,
            CONFIG,
        );
        assert_eq!(legacy, wire);
    }

    #[test]
    fn streaming_unknown_length_chunked_matches_legacy() {
        let steps = || {
            [
                Step::Data(b"the first read"),
                Step::Data(b"a second, longer read of content"),
                Step::Data(b"3"),
                Step::Eof,
            ]
        };
        let (legacy, legacy_trailers) =
            legacy_wire(Body::new_streaming(ScriptSource::new(steps()), None));
        let (wire, trailers) = write_wire(
            Body::new_streaming(ScriptSource::new(steps()), None),
            BodyFraming::Chunked { keep_open: false },
            CONFIG,
        );
        assert_eq!(legacy, wire);
        assert!(legacy_trailers.is_none() && trailers.is_none());
    }

    #[test]
    fn trailers_match_legacy() {
        let body = || {
            let mut trailers = Headers::new();
            trailers.insert("x-checksum", "abc123");
            Body::new_with_trailers(
                ScriptSource::new([Step::Data(b"hello"), Step::Eof]).with_trailers(trailers),
                None,
            )
        };
        let (legacy, legacy_trailers) = legacy_wire(body());
        let (wire, trailers) =
            write_wire(body(), BodyFraming::Chunked { keep_open: false }, CONFIG);
        assert_eq!(legacy, wire);
        assert_eq!(
            legacy_trailers.unwrap().get_str("x-checksum"),
            trailers.unwrap().get_str("x-checksum")
        );
    }
}

mod polling {
    //! Poll-level behavior: flush-when-source-pending and cooperative yielding.
    use super::*;
    use pretty_assertions::assert_eq;
    use std::{
        future::Future,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Wake, Waker},
    };

    struct CountWaker(AtomicUsize);
    impl Wake for CountWaker {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn pending_source_flushes_produced_content() {
        let wake_count = Arc::new(CountWaker(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&wake_count));
        let mut cx = Context::from_waker(&waker);

        let body = Body::new_streaming(
            ScriptSource::new([Step::Data(b"hello"), Step::Pending, Step::Eof]),
            None,
        );
        let mut out = Vec::new();
        let mut sink = BufWriter::new_with_buffer(Vec::with_capacity(512), &mut out, 2048);
        let mut future =
            Box::pin(body.write_into(&mut sink, BodyFraming::Chunked { keep_open: false }, CONFIG));

        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(future);
        drop(sink);
        // the already-produced chunk reached the transport while the source was pending
        assert_eq!(out, b"5\r\nhello\r\n");
    }

    #[test]
    fn yields_after_loops_per_yield_reads() {
        let wake_count = Arc::new(CountWaker(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&wake_count));
        let mut cx = Context::from_waker(&waker);

        let config = HttpConfig::default().with_copy_loops_per_yield(2);
        let body = Body::new_streaming(
            ScriptSource::new([
                Step::Data(b"one"),
                Step::Data(b"two"),
                Step::Data(b"three"),
                Step::Eof,
            ]),
            None,
        );
        let mut out = Vec::new();
        let mut sink = BufWriter::new_with_buffer(Vec::with_capacity(512), &mut out, 2048);
        let mut future = Box::pin(body.write_into(
            &mut sink,
            BodyFraming::Chunked { keep_open: false },
            &config,
        ));

        // two reads, then a cooperative yield with a self-wake
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(wake_count.0.load(Ordering::SeqCst), 1);
        // resumption completes the remaining read + eof
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }
}

#[test]
fn hex_width_matches_formatting() {
    for n in [0, 1, 15, 16, 255, 256, 4095, 4096, 65535, 65536] {
        assert_eq!(hex_width(n), format!("{n:X}").len(), "hex_width({n})");
    }
}
