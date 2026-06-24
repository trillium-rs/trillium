use futures_lite::{AsyncBufRead, AsyncRead, AsyncWrite, io::BufReader, ready};
use std::{
    future::Future,
    io::{ErrorKind, Result},
    pin::{Pin, pin},
    task::{Context, Poll},
};

/// copy from the `reader` to the `writer`, yielding back to the runtime every `loops_per_yield`
///
/// # Errors
///
/// This returns any io error encountered in reading or writing
#[doc(hidden)]
pub async fn copy<R, W>(reader: R, writer: W, loops_per_yield: usize) -> Result<u64>
where
    R: AsyncRead,
    W: AsyncWrite,
{
    struct CopyFuture<'r, 'w, R, W> {
        reader: BufReader<Pin<&'r mut R>>,
        writer: Pin<&'w mut W>,
        amt: u64,
        loops_per_yield: usize,
    }

    impl<R, W> Future for CopyFuture<'_, '_, R, W>
    where
        R: AsyncRead,
        W: AsyncWrite,
    {
        type Output = Result<u64>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            for loop_number in 0..self.loops_per_yield {
                log::trace!("copy loop number: {loop_number}");
                let CopyFuture {
                    reader,
                    writer,
                    amt,
                    ..
                } = &mut *self;

                let writer = Pin::new(writer);
                let mut reader = Pin::new(reader);
                let buffer = match reader.as_mut().poll_fill_buf(cx) {
                    Poll::Ready(buffer) => buffer?,
                    Poll::Pending => {
                        ready!(writer.poll_flush(cx))?;
                        return Poll::Pending;
                    }
                };
                if buffer.is_empty() {
                    ready!(writer.poll_flush(cx))?;
                    return Poll::Ready(Ok(self.amt));
                }

                let i = ready!(writer.poll_write(cx, buffer))?;
                if i == 0 {
                    return Poll::Ready(Err(ErrorKind::WriteZero.into()));
                }
                *amt += i as u64;
                reader.consume(i);
            }

            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    let reader = pin!(reader);
    let writer = pin!(writer);

    let future = CopyFuture {
        reader: BufReader::new(reader),
        writer,
        amt: 0,
        loops_per_yield,
    };
    future.await
}

#[cfg(test)]
mod tests {
    use super::copy;
    use futures_lite::{AsyncRead, AsyncWrite};
    use std::{
        collections::VecDeque,
        io::{ErrorKind, Result},
        mem,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
    };

    enum Step {
        Data(&'static [u8]),
        Pending,
        Eof,
    }

    struct ScriptReader(VecDeque<Step>);
    impl ScriptReader {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self(steps.into_iter().collect())
        }
    }
    impl AsyncRead for ScriptReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<Result<usize>> {
            match self.0.pop_front() {
                Some(Step::Data(data)) => {
                    let n = data.len().min(buf.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    Poll::Ready(Ok(n))
                }
                Some(Step::Pending) => Poll::Pending,
                Some(Step::Eof) | None => Poll::Ready(Ok(0)),
            }
        }
    }

    #[derive(Default, Clone, Copy)]
    enum FlushBehavior {
        #[default]
        Ok,
        Pending,
        Error,
    }

    /// Models `BufWriter`: writes are absorbed and only become visible on flush.
    #[derive(Default)]
    struct FlushRecorder {
        pending: Vec<u8>,
        flushed: Vec<u8>,
        flush_calls: usize,
        flush_behavior: FlushBehavior,
    }
    impl AsyncWrite for FlushRecorder {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize>> {
            self.pending.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
            self.flush_calls += 1;
            match self.flush_behavior {
                FlushBehavior::Ok => {
                    let mut pending = mem::take(&mut self.pending);
                    self.flushed.append(&mut pending);
                    Poll::Ready(Ok(()))
                }
                FlushBehavior::Pending => Poll::Pending,
                FlushBehavior::Error => Poll::Ready(Err(ErrorKind::Other.into())),
            }
        }

        fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
            self.poll_flush(cx)
        }
    }

    struct CountWaker(AtomicUsize);
    impl Wake for CountWaker {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn poll_copy_once(reader: ScriptReader, writer: &mut FlushRecorder) -> Poll<Result<u64>> {
        let waker = Waker::from(Arc::new(CountWaker(AtomicUsize::new(0))));
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(copy(reader, writer, 16));
        future.as_mut().poll(&mut cx)
    }

    // The regression: a small write absorbed by the buffered writer must be
    // flushed out when the reader has nothing more to offer yet, rather than
    // stalling until the next overflow or EOF. Before the fix `flushed` is empty.
    #[test]
    fn flushes_buffered_data_when_reader_idle() {
        let mut writer = FlushRecorder::default();
        let poll = poll_copy_once(
            ScriptReader::new([Step::Data(b"hello"), Step::Pending]),
            &mut writer,
        );
        assert!(poll.is_pending());
        assert_eq!(writer.flushed, b"hello");
        assert_eq!(writer.flush_calls, 1);
    }

    #[test]
    fn idle_flush_pending_yields_pending() {
        let mut writer = FlushRecorder {
            flush_behavior: FlushBehavior::Pending,
            ..Default::default()
        };
        let poll = poll_copy_once(
            ScriptReader::new([Step::Data(b"hi"), Step::Pending]),
            &mut writer,
        );
        assert!(poll.is_pending());
        assert!(writer.flushed.is_empty());
        assert_eq!(writer.pending, b"hi");
        assert_eq!(writer.flush_calls, 1);
    }

    #[test]
    fn idle_flush_error_propagates() {
        let mut writer = FlushRecorder {
            flush_behavior: FlushBehavior::Error,
            ..Default::default()
        };
        let poll = poll_copy_once(
            ScriptReader::new([Step::Data(b"hi"), Step::Pending]),
            &mut writer,
        );
        match poll {
            Poll::Ready(Err(e)) => assert_eq!(e.kind(), ErrorKind::Other),
            other => panic!("expected ready error, got {other:?}"),
        }
    }

    #[test]
    fn eof_flushes_and_returns_total() {
        let mut writer = FlushRecorder::default();
        let poll = poll_copy_once(
            ScriptReader::new([Step::Data(b"hello"), Step::Data(b" world"), Step::Eof]),
            &mut writer,
        );
        match poll {
            Poll::Ready(Ok(amt)) => assert_eq!(amt, 11),
            other => panic!("expected ready total, got {other:?}"),
        }
        assert_eq!(writer.flushed, b"hello world");
    }

    // Guards against "fix" regressions that flush on every iteration: while data
    // is flowing there should be exactly one flush, at EOF.
    #[test]
    fn continuous_data_flushes_only_at_eof() {
        let mut writer = FlushRecorder::default();
        let _ = poll_copy_once(
            ScriptReader::new([
                Step::Data(b"a"),
                Step::Data(b"b"),
                Step::Data(b"c"),
                Step::Eof,
            ]),
            &mut writer,
        );
        assert_eq!(writer.flushed, b"abc");
        assert_eq!(writer.flush_calls, 1);
    }
}
