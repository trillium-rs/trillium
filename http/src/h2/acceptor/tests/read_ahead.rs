//! Read batching: frames the peer sends together are read from the transport together.

use super::fixture::{decode_frames, noop_waker};
use crate::{
    HttpContext,
    h2::{acceptor::recv::CLIENT_PREFACE, connection::H2Connection, frame::Frame},
};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use trillium_testing::TestTransport;

/// Counts the reads that returned bytes.
struct CountsReads(TestTransport, Arc<AtomicUsize>);

impl AsyncRead for CountsReads {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.0).poll_read(cx, buf);
        if matches!(poll, Poll::Ready(Ok(n)) if n > 0) {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
        poll
    }
}

impl AsyncWrite for CountsReads {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

#[test]
fn frames_that_arrive_together_are_read_together() {
    const PINGS: u8 = 5;
    let (transport, peer) = TestTransport::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let connection = H2Connection::new(Arc::new(HttpContext::new()));
    let mut driver = connection.run(CountsReads(transport, Arc::clone(&reads)));

    let mut burst = CLIENT_PREFACE.to_vec();
    // An empty SETTINGS frame, then PINGs, each a 9-byte header plus an 8-byte payload.
    burst.extend_from_slice(&[0, 0, 0, 0x4, 0, 0, 0, 0, 0]);
    for n in 1..=PINGS {
        burst.extend_from_slice(&[0, 0, 8, 0x6, 0, 0, 0, 0, 0]);
        burst.extend_from_slice(&[n; 8]);
    }
    peer.write_all(&burst);

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    for _ in 0..4 {
        let _ = driver.drive(&mut cx);
    }

    let acks = decode_frames(&peer.snapshot())
        .into_iter()
        .filter(|frame| matches!(frame, Frame::Ping { ack: true, .. }))
        .count();
    assert_eq!(acks, usize::from(PINGS));
    assert_eq!(reads.load(Ordering::Relaxed), 1);
}
