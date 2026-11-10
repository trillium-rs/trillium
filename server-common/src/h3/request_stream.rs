//! The transport behind each HTTP/3 request stream, and what dropping it means.
//!
//! A QUIC send stream finishes when dropped, which presents whatever was written as complete. That
//! is right for a response trillium-http finished, which it closes explicitly, but wrong for one
//! that was abandoned: a failed body, a panic, a canceled task. [`RequestStream`] resets a stream
//! dropped without finishing, with `H3_INTERNAL_ERROR`.
//!
//! A stream handed to an upgrade handler or WebTransport belongs to code that conventionally ends
//! it by dropping it, so there a drop finishes the stream as before, unless a panic is unwinding.

use super::priority::PrioritizedStream;
use crate::{BoxedBidiStream, QuicTransportBidi, QuicTransportReceive, QuicTransportSend};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    any::Any,
    borrow::Cow,
    fmt, io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use trillium::Transport;
use trillium_http::{PeerGone, h3::H3ErrorCode};

pub(crate) struct RequestStream {
    inner: PrioritizedStream<BoxedBidiStream>,
    ended: bool,
    handed_off: bool,
}

impl fmt::Debug for RequestStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestStream")
            .field("ended", &self.ended)
            .field("handed_off", &self.handed_off)
            .finish_non_exhaustive()
    }
}

impl RequestStream {
    pub(crate) fn new(inner: PrioritizedStream<BoxedBidiStream>) -> Self {
        Self {
            inner,
            ended: false,
            handed_off: false,
        }
    }

    /// Mark the stream as handed to code that ends it by dropping it: a drop then finishes the
    /// stream, unless a panic is unwinding.
    pub(crate) fn hand_off(&mut self) {
        self.handed_off = true;
    }

    /// The request stream behind an erased transport, if it is one.
    pub(crate) fn from_transport(transport: &mut dyn Transport) -> Option<&mut Self> {
        (transport as &mut dyn Any).downcast_mut()
    }
}

impl Drop for RequestStream {
    fn drop(&mut self) {
        if self.ended || (self.handed_off && !std::thread::panicking()) {
            return;
        }
        log::debug!("h3 request stream dropped unfinished; resetting with H3_INTERNAL_ERROR");
        self.inner.reset(H3ErrorCode::InternalError.into());
    }
}

impl AsyncRead for RequestStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for RequestStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let closed = futures_lite::ready!(Pin::new(&mut self.inner).poll_close(cx));
        self.ended |= closed.is_ok();
        Poll::Ready(closed)
    }
}

impl QuicTransportReceive for RequestStream {
    fn stop(&mut self, code: u64) {
        self.inner.stop(code);
    }
}

impl QuicTransportSend for RequestStream {
    fn reset(&mut self, code: u64) {
        self.ended = true;
        self.inner.reset(code);
    }

    fn stopped(&self) -> Option<PeerGone> {
        self.inner.stopped()
    }

    fn set_priority(&mut self, priority: i32) {
        self.inner.set_priority(priority);
    }
}

impl Transport for RequestStream {
    fn set_linger(&mut self, linger: Option<Duration>) -> io::Result<()> {
        self.inner.set_linger(linger)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> io::Result<()> {
        self.inner.set_nodelay(nodelay)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> io::Result<()> {
        self.inner.set_ip_ttl(ttl)
    }

    fn peer_addr(&self) -> io::Result<Option<SocketAddr>> {
        self.inner.peer_addr()
    }

    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        self.inner.negotiated_alpn()
    }
}

impl QuicTransportBidi for RequestStream {}

#[cfg(test)]
mod tests {
    use super::{PrioritizedStream, RequestStream};
    use crate::{
        QuicTransportBidi, QuicTransportReceive, QuicTransportSend, h3::priority::PriorityRegistry,
    };
    use futures_lite::{AsyncRead, AsyncWrite, AsyncWriteExt, future::block_on};
    use std::{
        io,
        panic::{AssertUnwindSafe, catch_unwind},
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
    };
    use trillium::Transport;

    /// Records the stream-ending signals sent through it.
    struct Recording(Arc<Mutex<Vec<String>>>);

    impl AsyncRead for Recording {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(0))
        }
    }

    impl AsyncWrite for Recording {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.0.lock().unwrap().push("finish".into());
            Poll::Ready(Ok(()))
        }
    }

    impl QuicTransportReceive for Recording {
        fn stop(&mut self, _: u64) {}
    }

    impl QuicTransportSend for Recording {
        fn reset(&mut self, code: u64) {
            self.0.lock().unwrap().push(format!("reset {code:#x}"));
        }
    }

    impl Transport for Recording {}
    impl QuicTransportBidi for Recording {}

    fn new_stream() -> (RequestStream, Arc<Mutex<Vec<String>>>) {
        let signals = Arc::new(Mutex::new(Vec::new()));
        let registry = PriorityRegistry::default();
        let inner = PrioritizedStream::new(
            Box::new(Recording(Arc::clone(&signals))) as crate::BoxedBidiStream,
            registry.register(0),
            0,
        );
        (RequestStream::new(inner), signals)
    }

    fn dropped_while_panicking(stream: RequestStream) {
        let result = catch_unwind(AssertUnwindSafe(move || {
            let _stream = stream;
            panic!("handler panicked");
        }));
        assert!(result.is_err());
    }

    fn signals(recorded: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        recorded.lock().unwrap().clone()
    }

    #[test]
    fn unfinished_stream_resets_on_drop() {
        let (stream, recorded) = new_stream();
        drop(stream);
        assert_eq!(signals(&recorded), ["reset 0x102"]);

        let (stream, recorded) = new_stream();
        dropped_while_panicking(stream);
        assert_eq!(signals(&recorded), ["reset 0x102"]);
    }

    #[test]
    fn finished_or_reset_stream_drops_quietly() {
        let (mut stream, recorded) = new_stream();
        block_on(stream.close()).unwrap();
        drop(stream);
        assert_eq!(signals(&recorded), ["finish"]);

        let (mut stream, recorded) = new_stream();
        stream.reset(7);
        drop(stream);
        assert_eq!(signals(&recorded), ["reset 0x7"]);
    }

    #[test]
    fn handed_off_stream_finishes_on_drop_unless_panicking() {
        let (mut stream, recorded) = new_stream();
        stream.hand_off();
        drop(stream);
        assert_eq!(signals(&recorded), Vec::<String>::new());

        let (mut stream, recorded) = new_stream();
        stream.hand_off();
        dropped_while_panicking(stream);
        assert_eq!(signals(&recorded), ["reset 0x102"]);
    }
}
