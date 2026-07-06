use super::*;
use std::{
    io::{self, ErrorKind},
    pin::Pin,
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
