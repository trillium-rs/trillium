//! HTTP/2 specific dispatch for runtime adapters.
//!
//! Entry point used by [`running_config`][crate::running_config]: when the TLS acceptor
//! signals `h2` via ALPN, or when a connection (cleartext or TLS-without-ALPN-h2) presents
//! the HTTP/2 client preface, the adapter hands the transport to [`run_h2`]. This module
//! owns the per-connection driver loop and per-stream task spawning that mirrors
//! [`h3::run_h3`][crate::h3::run_h3].

use crate::{ArcHandler, Runtime};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    io,
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium::{Handler, Transport, Upgrade};
use trillium_http::{
    HttpContext,
    h2::{H2Connection, H2Transport},
};

/// HTTP/2 client connection preface (RFC 9113 §3.4). The first 24 bytes every HTTP/2 client
/// sends before any frames; the prior-knowledge dispatch path peeks the (cleartext or
/// post-TLS) stream for these bytes to decide between HTTP/1.1 and HTTP/2.
pub(crate) const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Drive an HTTP/2 connection end-to-end: construct the [`H2Connection`], run its driver
/// loop, and spawn a per-stream task running the user handler for every emitted [`Conn`].
///
/// `peer_ip` and `is_secure` are populated onto each per-stream [`Conn`] before the handler
/// runs, matching [`crate::running_config`]'s HTTP/1.1 path. `is_secure` reflects the
/// underlying transport: cleartext h2c sets it to `false`; ALPN-negotiated h2 and
/// TLS-prior-knowledge h2 both set it to whatever the TLS acceptor reports.
pub(crate) async fn run_h2<T>(
    transport: T,
    context: Arc<HttpContext>,
    handler: ArcHandler<impl Handler>,
    runtime: Runtime,
    peer_ip: Option<IpAddr>,
    is_secure: bool,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    let h2 = H2Connection::new(context);
    let mut driver = h2.clone().run(transport);

    while let Some(result) = driver.next().await {
        match result {
            Ok(conn) => {
                let stream_id = conn.h2_stream_id();
                log::trace!("run_h2: spawning handler task for stream {stream_id:?}");
                let handler = handler.clone();
                runtime.spawn(async move {
                    let inner_handler = handler.clone();
                    let result = H2Connection::process_inbound(conn, |mut conn| async move {
                        let handler = &inner_handler;
                        conn.set_peer_ip(peer_ip);
                        conn.set_secure(is_secure);
                        let conn = handler.run(conn.into()).await;
                        let conn = handler.before_send(conn).await;
                        conn.into_inner::<H2Transport>()
                    })
                    .await;

                    match result {
                        Ok(conn) if conn.should_upgrade() => {
                            let upgrade = Upgrade::from(conn);
                            if handler.has_upgrade(&upgrade) {
                                log::debug!("upgrading h2 stream");
                                handler.upgrade(upgrade).await;
                            } else {
                                log::error!("h2 upgrade specified but no upgrade handler provided");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            log::debug!("h2 stream error: {e}");
                        }
                    }
                });
            }
            Err(e) => {
                log::debug!("h2 connection error: {e}");
                break;
            }
        }
    }
    log::trace!("run_h2: driver exhausted, connection done");
}

/// A TCP transport that first serves a pre-peeked byte prefix before reading from its wrapped
/// inner transport.
///
/// Used by the HTTP/2 prior-knowledge path in [`crate::running_config`] (both cleartext
/// h2c and TLS-without-ALPN-h2): the adapter peeks the first 24 bytes of the post-acceptor
/// transport to compare against [`CLIENT_PREFACE`]; if they match, we hand the original
/// transport *with those bytes prepended* to [`run_h2`] so the driver's preface-reading
/// state can consume them without re-reading from the wire.
///
/// Forwards [`Transport`] trait methods to the wrapped inner transport so socket options and
/// peer-addr queries behave the same as they would without the wrapper.
#[derive(Debug)]
pub(crate) struct Prefixed<T> {
    prefix: Vec<u8>,
    offset: usize,
    inner: T,
}

impl<T> Prefixed<T> {
    pub(crate) fn new(prefix: Vec<u8>, inner: T) -> Self {
        Self {
            prefix,
            offset: 0,
            inner,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Prefixed<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.offset < this.prefix.len() {
            let take = (this.prefix.len() - this.offset).min(buf.len());
            buf[..take].copy_from_slice(&this.prefix[this.offset..this.offset + take]);
            this.offset += take;
            if this.offset >= this.prefix.len() {
                // Drop the peeked bytes once replayed — no further references to them, and a
                // long-lived connection shouldn't keep the allocation around.
                this.prefix = Vec::new();
            }
            return Poll::Ready(Ok(take));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Prefixed<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

impl<T: Transport> Transport for Prefixed<T> {
    fn set_linger(&mut self, linger: Option<std::time::Duration>) -> io::Result<()> {
        self.inner.set_linger(linger)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> io::Result<()> {
        self.inner.set_nodelay(nodelay)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> io::Result<()> {
        self.inner.set_ip_ttl(ttl)
    }

    fn peer_addr(&self) -> io::Result<Option<std::net::SocketAddr>> {
        self.inner.peer_addr()
    }

    fn negotiated_alpn(&self) -> Option<std::borrow::Cow<'_, [u8]>> {
        self.inner.negotiated_alpn()
    }
}
