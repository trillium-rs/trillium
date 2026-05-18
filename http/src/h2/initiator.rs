//! Client-side wrapper around [`H2Driver`]: the background-task future a trillium client
//! spawns after [`H2Connection::run_client`]. Polls the driver until the connection closes
//! and resolves with the final outcome.
//!
//! Unlike the server-side [`H2Driver::next`] loop, the client has no `Conn`-emission
//! cadence — streams are initiated locally and responses are awaited per-stream, not
//! dequeued from the driver. An `Action::Emit` on a client-role driver would mean a peer-
//! initiated stream (server push), which is unreachable because our advertised SETTINGS
//! disable push; the guard here logs + drops just in case.
//!
//! [`H2Connection::run_client`]: super::H2Connection::run_client

use super::{H2Error, acceptor::H2Driver};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Background-task future for a client-role HTTP/2 connection. Resolves with `Ok(())` on
/// graceful close or `Err(H2Error)` on a protocol/I/O failure.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct H2Initiator<T> {
    driver: H2Driver<T>,
}

impl<T> H2Initiator<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    pub(super) fn new(driver: H2Driver<T>) -> Self {
        Self { driver }
    }
}

impl<T> Future for H2Initiator<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    type Output = Result<(), H2Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match self.driver.drive(cx) {
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(Some(Ok(_conn))) => {
                    // Push is disabled in our advertised SETTINGS, so a peer-initiated
                    // stream shouldn't reach us. The driver's own `unreachable!` guards
                    // cover the spec-violation case; this is a belt-and-suspenders log.
                    log::error!(
                        "h2 client driver: unexpected peer-initiated stream — dropping conn"
                    );
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
