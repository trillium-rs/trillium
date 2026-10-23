use crate::WebSocketConn;
use futures_lite::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug)]
pub(crate) enum Direction<I, O> {
    Inbound(I),
    Outbound(O),
}

/// Polls the conn and the outbound stream in a random order, so that neither starves the other.
///
/// Returns `None` when either one ends.
pub(crate) fn poll_next_direction<O: Stream>(
    conn: &mut WebSocketConn,
    mut outbound: Pin<&mut O>,
    cx: &mut Context<'_>,
) -> Poll<Option<Direction<<WebSocketConn as Stream>::Item, O::Item>>> {
    macro_rules! poll_inbound {
        () => {
            match Pin::new(&mut *conn).poll_next(cx) {
                Poll::Ready(Some(t)) => return Poll::Ready(Some(Direction::Inbound(t))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => (),
            }
        };
    }
    macro_rules! poll_outbound {
        () => {
            match outbound.as_mut().poll_next(cx) {
                Poll::Ready(Some(t)) => return Poll::Ready(Some(Direction::Outbound(t))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => (),
            }
        };
    }

    // Retained only for the deprecated `WebSocketConn::take_inbound_stream`; remove it at the next
    // breaking release. A `connect` that took the inbound stream has moved it into the outbound
    // stream, so only that is polled, and queued sends are written out before parking.
    if conn.inbound_taken() {
        poll_outbound!();
        if let Poll::Ready(Err(e)) = conn.poll_write_out(cx) {
            log::debug!("websocket write error: {e}");
        }
        return Poll::Pending;
    }

    if fastrand::bool() {
        poll_inbound!();
        poll_outbound!();
    } else {
        poll_outbound!();
        poll_inbound!();
    }

    Poll::Pending
}
