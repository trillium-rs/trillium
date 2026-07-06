use super::*;
use std::{
    io::{self, ErrorKind},
    pin::Pin,
    sync::{Mutex, OnceLock},
    task::{Context, Poll},
};

/// An `AsyncWrite` whose first write fails, so a stream pump errors immediately on its
/// mandatory stream-type byte rather than parking.
struct FailingWriter;
impl AsyncWrite for FailingWriter {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::new(ErrorKind::Other, "writer failed")))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn h3() -> Arc<H3Connection> {
    H3Connection::new(Arc::new(HttpContext::default()))
}

// A mandatory stream pump that errors must shut the whole connection down, so a pooling
// caller reading `swansong().state().is_running()` evicts it instead of reusing a
// connection whose control/QPACK machinery is dead.

#[test]
fn run_decoder_shuts_down_on_writer_error() {
    let h3 = h3();
    assert!(h3.swansong().state().is_running());
    let result = futures_lite::future::block_on(h3.run_decoder(FailingWriter));
    assert!(result.is_err());
    assert!(!h3.swansong().state().is_running());
}

#[test]
fn run_encoder_shuts_down_on_writer_error() {
    let h3 = h3();
    assert!(h3.swansong().state().is_running());
    let result = futures_lite::future::block_on(h3.run_encoder(FailingWriter));
    assert!(result.is_err());
    assert!(!h3.swansong().state().is_running());
}

#[test]
fn run_outbound_control_shuts_down_on_writer_error() {
    let h3 = h3();
    assert!(h3.swansong().state().is_running());
    let result = futures_lite::future::block_on(h3.run_outbound_control(FailingWriter));
    assert!(result.is_err());
    assert!(!h3.swansong().state().is_running());
}

/// A transport with canned inbound bytes that records everything written to it.
struct CannedTransport {
    input: futures_lite::io::Cursor<Vec<u8>>,
    written: Arc<Mutex<Vec<u8>>>,
}

impl AsyncRead for CannedTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.input).poll_read(cx, buf)
    }
}

impl AsyncWrite for CannedTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.written.lock().unwrap().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// A GET request as a complete HEADERS frame, encoded by a throwaway connection whose
/// dynamic table has no capacity — so the section is static/literal-only and decodes
/// without any encoder-stream input.
fn encoded_get_request(stream_id: u64) -> Vec<u8> {
    use crate::headers::qpack::{FieldSection, PseudoHeaders};

    let mut pseudo_headers = PseudoHeaders::default()
        .with_method(crate::Method::Get)
        .with_authority("example.com");
    pseudo_headers.set_path(Some("/")).set_scheme(Some("https"));
    let headers = crate::Headers::new();
    let field_section = FieldSection::new(pseudo_headers, &headers);

    let mut buf = Vec::new();
    h3().encode_field_section_framed(&field_section, &mut buf, stream_id)
        .unwrap();
    buf
}

#[test]
fn with_request_rejection_refuses_the_request_without_responding() {
    let h3 = h3();
    let written = Arc::new(Mutex::new(Vec::new()));
    let reset_code = Arc::new(OnceLock::new());
    let transport = CannedTransport {
        input: futures_lite::io::Cursor::new(encoded_get_request(1)),
        written: written.clone(),
    };

    let result = futures_lite::future::block_on(async {
        h3.process_inbound_bidi(transport, |conn| async move { conn }, 1)
            .with_request_rejection()
            .with_reset({
                let reset_code = reset_code.clone();
                move |_, code| reset_code.set(code).unwrap()
            })
            .await
    });

    assert!(matches!(
        result,
        Err(H3Error::Protocol(H3ErrorCode::StreamCreationError))
    ));
    assert_eq!(reset_code.get(), Some(&H3ErrorCode::StreamCreationError));
    assert!(
        written.lock().unwrap().is_empty(),
        "a rejected request stream must not receive a response"
    );
}

#[test]
fn without_request_rejection_the_same_stream_is_served() {
    let h3 = h3();
    let written = Arc::new(Mutex::new(Vec::new()));
    let transport = CannedTransport {
        input: futures_lite::io::Cursor::new(encoded_get_request(1)),
        written: written.clone(),
    };

    let result = futures_lite::future::block_on(async {
        h3.process_inbound_bidi(transport, |conn| async move { conn }, 1)
            .with_reset(|_, _| panic!("reset hook must not fire for a served request"))
            .await
    });

    assert!(matches!(result, Ok(H3StreamResult::Request(_))));
    assert!(
        !written.lock().unwrap().is_empty(),
        "a served request receives a response"
    );
}
