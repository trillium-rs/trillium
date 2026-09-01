use crate::{Buffer, Conn, ProtocolSession, Version};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Upper bound on bytes buffered from a peer while a handler runs without
/// reading its request. Beyond this, the socket backs up instead of memory.
const PROBE_WINDOW_CAP: usize = 16 * 1024;

/// A future that resolves when the peer abandons a single HTTP/3 stream.
///
/// HTTP/3 has no connection driver inside trillium-http — QUIC stream resets, `STOP_SENDING`,
/// and connection loss are all invisible at the `AsyncRead` + `AsyncWrite` seam the h3 code is
/// written against. The runtime adapter supplies this future at stream accept, the same way it
/// supplies the stream-reset closure.
pub type PeerGone = Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>;

/// Resolves once the peer has abandoned this request, by whatever means the protocol makes
/// observable.
///
/// - **HTTP/1.x** — reads the transport; end-of-file or a transport error resolves. Bytes read
///   accumulate on `buffer` (where the inbound state machine picks them up) until it holds
///   `read_allowance` bytes, after which the probe goes dormant. A half-closed peer that shut down
///   its write side but still reads is indistinguishable from a departed one and counts as gone.
/// - **HTTP/2** — the stream is reset, both halves are complete, or the connection is torn down.
///   Never reads the transport, so `read_allowance` is unused.
/// - **HTTP/3** — `peer_gone` resolves: `STOP_SENDING`, stream reset, or connection loss. Never
///   reads the transport. `None` when the runtime adapter supplied no future, in which case this
///   never resolves.
///
/// Reading the transport is only correct for h1. On h2 and h3 the peer half-closes its send
/// side as a matter of course — an h2 request carries `END_STREAM` on its HEADERS and an h3
/// client FINs its half of the bidi stream — so an EOF there is the *normal* state of a live
/// request, not evidence of anything.
pub(crate) fn poll_peer_gone<T: AsyncRead + Unpin>(
    session: &ProtocolSession,
    version: Version,
    buffer: &mut Buffer,
    transport: &mut T,
    peer_gone: Option<&mut PeerGone>,
    read_allowance: usize,
    cx: &mut Context<'_>,
) -> Poll<()> {
    match session {
        ProtocolSession::Http2 {
            connection,
            stream_id,
        } => return connection.poll_stream_closed(*stream_id, cx),

        ProtocolSession::Http3 { .. } => {
            return match peer_gone {
                Some(peer_gone) => peer_gone.as_mut().poll(cx),
                None => Poll::Pending,
            };
        }

        ProtocolSession::Http1 => {}
    }

    // A synthetic conn has no peer to depart, and `Http1` is also the session for h3 conns
    // before their session is attached; neither should be probed by reading.
    if !matches!(
        version,
        Version::Http0_9 | Version::Http1_0 | Version::Http1_1
    ) {
        return Poll::Pending;
    }

    loop {
        // A peer that pipelines faster than handlers consume is put under backpressure rather
        // than buffered without bound; a connection this busy is definitionally alive.
        let want = read_allowance.saturating_sub(buffer.live_len());
        if want == 0 {
            return Poll::Pending;
        }

        match Pin::new(&mut *transport).poll_read(cx, buffer.window(want)) {
            Poll::Ready(Ok(0) | Err(_)) => return Poll::Ready(()),
            Poll::Ready(Ok(n)) => buffer.advance(n),
            Poll::Pending => return Poll::Pending,
        }
    }
}

pub(crate) struct LivenessFut<'a, T>(&'a mut Conn<T>);

impl<'a, T> LivenessFut<'a, T> {
    pub(crate) fn new(conn: &'a mut Conn<T>) -> Self {
        Self(conn)
    }
}

impl<T> Future for LivenessFut<'_, T>
where
    T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let LivenessFut(Conn {
            buffer,
            transport,
            protocol_session,
            version,
            peer_gone,
            ..
        }) = &mut *self;

        poll_peer_gone(
            protocol_session,
            *version,
            buffer,
            transport,
            peer_gone.as_mut(),
            PROBE_WINDOW_CAP,
            cx,
        )
    }
}

pub(crate) struct CancelOnDisconnect<'a, Fut, T>(
    pub(crate) &'a mut Conn<T>,
    pub(crate) Pin<&'a mut Fut>,
);
impl<'a, Fut, T> Future for CancelOnDisconnect<'a, Fut, T>
where
    Fut: Future + Send + 'a,
    T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = Option<Fut::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let CancelOnDisconnect(conn, fut) = &mut *self;
        let fut_poll = fut.as_mut().poll(cx);
        let disconnect = Pin::new(&mut LivenessFut(conn)).poll(cx);
        match (fut_poll, disconnect) {
            (Poll::Ready(output), _) => Poll::Ready(Some(output)),
            (Poll::Pending, Poll::Ready(())) => Poll::Ready(None),
            (Poll::Pending, Poll::Pending) => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpContext, ProtocolSession, h3::H3Connection};
    use futures_lite::io::Cursor;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    /// A future that resolves only once `flag` is set, standing in for the runtime adapter's
    /// QUIC abandonment signal.
    fn gated(flag: Arc<AtomicBool>) -> PeerGone {
        Box::pin(std::future::poll_fn(move |cx| {
            if flag.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }))
    }

    fn h3_session() -> ProtocolSession {
        ProtocolSession::Http3 {
            connection: H3Connection::new(Arc::new(HttpContext::new())),
            stream_id: 0,
        }
    }

    /// A transport at EOF is the *normal* state of a live h3 request — the client finishes its
    /// half of the bidi stream once the request is complete — so the dispatcher must not read
    /// it. With no abandonment signal supplied, there is nothing to report.
    #[test]
    fn h3_ignores_an_eof_transport() {
        let mut transport = Cursor::new(Vec::new());
        let mut buffer = Buffer::default();
        let polled = futures_lite::future::block_on(std::future::poll_fn(|cx| {
            Poll::Ready(poll_peer_gone(
                &h3_session(),
                Version::Http3,
                &mut buffer,
                &mut transport,
                None,
                16 * 1024,
                cx,
            ))
        }));

        assert!(
            polled.is_pending(),
            "an h3 client that finished sending its request has not departed"
        );
    }

    #[test]
    fn h3_reports_departure_only_once_signalled() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut peer_gone = gated(flag.clone());
        let mut transport = Cursor::new(Vec::new());
        let mut buffer = Buffer::default();
        let session = h3_session();

        let poll =
            |peer_gone: &mut PeerGone, transport: &mut Cursor<Vec<u8>>, buffer: &mut Buffer| {
                futures_lite::future::block_on(std::future::poll_fn(|cx| {
                    Poll::Ready(poll_peer_gone(
                        &session,
                        Version::Http3,
                        buffer,
                        transport,
                        Some(peer_gone),
                        16 * 1024,
                        cx,
                    ))
                }))
            };

        assert!(poll(&mut peer_gone, &mut transport, &mut buffer).is_pending());

        flag.store(true, Ordering::SeqCst);
        assert!(
            poll(&mut peer_gone, &mut transport, &mut buffer).is_ready(),
            "once the adapter signals abandonment, the probe must resolve"
        );
    }
}
