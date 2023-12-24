use futures_lite::Stream;
use std::{
    fmt::{Debug, Formatter, Result},
    pin::Pin,
    task::{Context, Poll},
};
pin_project_lite::pin_project! {


pub(crate) struct BidirectionalStream<I, O> {
    pub(crate) inbound: Option<I>,
    #[pin]
    pub(crate) outbound: O,
}
}
impl<I, O> Debug for BidirectionalStream<I, O> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("BidirectionalStream")
            .field(
                "inbound",
                &match self.inbound {
                    Some(_) => "Some(_)",
                    None => "None",
                },
            )
            .field("outbound", &"..")
            .finish()
    }
}

#[derive(Debug)]
pub(crate) enum Direction<I, O> {
    Inbound(I),
    Outbound(O),
}

impl<I, O> Stream for BidirectionalStream<I, O>
where
    I: Stream + Unpin + Send + Sync + 'static,
    O: Stream + Send + Sync + 'static,
{
    type Item = Direction<I::Item, O::Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        macro_rules! poll_inbound {
            () => {
                if let Some(inbound) = &mut *this.inbound {
                    match Pin::new(inbound).poll_next(cx) {
                        Poll::Ready(Some(t)) => return Poll::Ready(Some(Direction::Inbound(t))),
                        Poll::Ready(None) => return Poll::Ready(None),
                        _ => (),
                    }
                }
            };
        }
        macro_rules! poll_outbound {
            () => {
                match this.outbound.poll_next(cx) {
                    Poll::Ready(Some(t)) => return Poll::Ready(Some(Direction::Outbound(t))),
                    Poll::Ready(None) => return Poll::Ready(None),
                    _ => (),
                }
            };
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
}
