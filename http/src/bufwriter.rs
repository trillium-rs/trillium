use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    fmt,
    io::{Error, ErrorKind, IoSlice, Result},
    pin::Pin,
    task::{Context, Poll, ready},
};
use trillium_macros::AsyncRead;

#[doc(hidden)]
#[derive(AsyncRead)]
pub struct BufWriter<W> {
    #[async_read]
    inner: W,
    buffer: Vec<u8>,
    buffer_flushed: usize,
    max_buffer_bytes: usize,
}

impl<W: AsyncWrite + Unpin> BufWriter<W> {
    #[doc(hidden)]
    pub fn new_with_buffer(buffer: Vec<u8>, inner: W, max_buffer_bytes: usize) -> Self {
        Self {
            inner,
            buffer,
            buffer_flushed: 0,
            max_buffer_bytes,
        }
    }

    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn inner(&mut self) -> &mut W {
        &mut self.inner
    }

    #[doc(hidden)]
    pub fn buffer_mut(&mut self) -> &mut Vec<u8> {
        &mut self.buffer
    }

    #[doc(hidden)]
    pub async fn copy_from<R: AsyncRead>(
        &mut self,
        reader: R,
        loops_per_yield: usize,
    ) -> Result<u64> {
        crate::copy::copy(reader, self, loops_per_yield).await
    }

    fn poll_flush_buf(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let Self {
            inner,
            buffer,
            buffer_flushed,
            ..
        } = &mut *self;

        while *buffer_flushed < buffer.len() {
            match ready!(Pin::new(&mut *inner).poll_write(cx, &buffer[*buffer_flushed..])) {
                Ok(0) => {
                    return Poll::Ready(Err(Error::new(
                        ErrorKind::WriteZero,
                        "Failed to write buffered data",
                    )));
                }
                Ok(n) => *buffer_flushed += n,
                Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
        }

        buffer.clear();
        *buffer_flushed = 0;
        Poll::Ready(Ok(()))
    }
}

impl<W> fmt::Debug for BufWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufWriter")
            .field("inner", &format_args!(".."))
            .field("buffer", &String::from_utf8_lossy(&self.buffer))
            .field("buffer_flushed", &self.buffer_flushed)
            .field("max_buffer_bytes", &self.max_buffer_bytes)
            .finish()
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for BufWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        additional: &[u8],
    ) -> Poll<Result<usize>> {
        let Self {
            inner,
            buffer,
            buffer_flushed,
            max_buffer_bytes,
        } = &mut *self;

        // Absorb into existing capacity if there's room
        if buffer.len() + additional.len() <= buffer.capacity() {
            buffer.extend_from_slice(additional);
            return Poll::Ready(Ok(additional.len()));
        }

        // Buffer would overflow capacity — flush pending via vectored write
        while *buffer_flushed < buffer.len() {
            let pending = &buffer[*buffer_flushed..];
            let written = ready!(
                Pin::new(&mut *inner)
                    .poll_write_vectored(cx, &[IoSlice::new(pending), IoSlice::new(additional)])
            )?;
            if written == 0 {
                return Poll::Ready(Err(Error::new(
                    ErrorKind::WriteZero,
                    "Failed to write buffered data",
                )));
            }
            let from_pending = written.min(pending.len());
            *buffer_flushed += from_pending;
            if *buffer_flushed >= buffer.len() {
                buffer.clear();
                *buffer_flushed = 0;
            }
            let from_additional = written - from_pending;
            if from_additional > 0 {
                return Poll::Ready(Ok(from_additional));
            }
        }

        // Buffer now empty after flushing — try absorbing again
        if additional.len() <= buffer.capacity() {
            buffer.extend_from_slice(additional);
            return Poll::Ready(Ok(additional.len()));
        }

        // Still too big for capacity — direct write, absorb remainder up to max
        let written = ready!(Pin::new(&mut *inner).poll_write(cx, additional))?;
        if written == 0 {
            return Poll::Ready(Err(Error::new(ErrorKind::WriteZero, "write returned 0")));
        }
        let remainder = &additional[written..];
        if !remainder.is_empty() && remainder.len() <= *max_buffer_bytes {
            buffer.extend_from_slice(remainder);
            return Poll::Ready(Ok(additional.len()));
        }
        Poll::Ready(Ok(written))
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        let Self {
            inner,
            buffer,
            buffer_flushed,
            ..
        } = &mut *self;
        let total: usize = bufs.iter().map(|b| b.len()).sum();
        if total == 0 {
            return Poll::Ready(Ok(0));
        }

        // Try to flush pending buffer alongside new slices
        let from_bufs = if *buffer_flushed < buffer.len() {
            let written = {
                let mut all = Vec::with_capacity(bufs.len() + 1);
                all.push(IoSlice::new(&buffer[*buffer_flushed..]));
                all.extend_from_slice(bufs);
                match Pin::new(&mut *inner).poll_write_vectored(cx, &all) {
                    Poll::Ready(Ok(0)) => {
                        return Poll::Ready(Err(Error::new(
                            ErrorKind::WriteZero,
                            "Failed to write buffered data",
                        )));
                    }
                    Poll::Ready(Ok(n)) => n,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => 0,
                }
            };

            let pending_len = buffer.len() - *buffer_flushed;
            let from_pending = written.min(pending_len);
            *buffer_flushed += from_pending;
            if *buffer_flushed >= buffer.len() {
                buffer.clear();
                *buffer_flushed = 0;
            }
            written - from_pending
        } else {
            match Pin::new(&mut *inner).poll_write_vectored(cx, bufs) {
                Poll::Ready(Ok(n)) => n,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => 0,
            }
        };

        // Absorb any unwritten bufs (no max_buffer_bytes limit for vectored)
        let mut skip = from_bufs;
        for buf in bufs {
            if skip >= buf.len() {
                skip -= buf.len();
            } else {
                buffer.extend_from_slice(&buf[skip..]);
                skip = 0;
            }
        }

        Poll::Ready(Ok(total))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::AsyncWriteExt;
    use pretty_assertions::assert_eq;
    #[derive(Default)]
    struct TestWrite {
        writes: Vec<Vec<u8>>,
        max_write: Option<usize>,
    }
    impl AsyncWrite for TestWrite {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize>> {
            let written = self.max_write.map_or(buf.len(), |mw| mw.min(buf.len()));
            self.writes.push(buf[..written].to_vec());
            Poll::Ready(Ok(written))
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<Result<usize>> {
            self.poll_write(cx, &bufs.iter().map(|s| &**s).collect::<Vec<_>>().concat())
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl TestWrite {
        fn new(max_write: Option<usize>) -> Self {
            Self {
                max_write,
                ..Self::default()
            }
        }

        fn data(&self) -> Vec<u8> {
            self.writes.concat()
        }
    }

    fn rand_bytes<const LEN: usize>() -> [u8; LEN] {
        std::array::from_fn(|_| fastrand::u8(..))
    }

    #[test]
    fn entire_content_shorter_than_capacity() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<90>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            bw.write_all(&data).await.unwrap();
            assert_eq!(bw.inner.writes.len(), 0);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
        });
    }

    #[test]
    fn longer_than_capacity_but_still_a_single_write() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            bw.write_all(&data).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
        });
    }

    #[test]
    fn multiple_writes() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<250>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            // 200 bytes > capacity, goes direct to inner
            bw.write_all(&data[..200]).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data[..200]]);
            // 50 bytes fits in capacity, absorbed into buffer
            bw.write_all(&data[200..]).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data[..200]]);
            // flush sends the buffered 50 bytes
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data[..200], &data[200..]]);
        });
    }

    #[test]
    fn overflow_is_vectored() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<101>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            bw.write_all(&data[..50]).await.unwrap();
            bw.write_all(&data[50..]).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
        });
    }

    #[test]
    fn max_write() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(Some(50));
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            bw.write_all(&data[..10]).await.unwrap();
            bw.write_all(&data[10..20]).await.unwrap();
            bw.write_all(&data[20..45]).await.unwrap();
            bw.write_all(&data[45..125]).await.unwrap();
            bw.write_all(&data[125..]).await.unwrap();
            // Small writes absorbed into buffer, then vectored flushes align to
            // inner's max_write (50), producing uniform 50-byte writes
            assert_eq!(
                &bw.inner.writes,
                &[&data[0..50], &data[50..100], &data[100..150]]
            );
            bw.flush().await.unwrap();
            assert_eq!(
                &bw.inner.writes,
                &[&data[0..50], &data[50..100], &data[100..150], &data[150..]]
            );
        });
    }

    #[test]
    fn write_boundary_is_exactly_buffer_len() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(Some(50));
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            bw.write_all(&data[..10]).await.unwrap();
            bw.write_all(&data[10..20]).await.unwrap();
            bw.write_all(&data[20..50]).await.unwrap();
            bw.write_all(&data[50..125]).await.unwrap();
            bw.write_all(&data[125..]).await.unwrap();
            // Same pattern as max_write — buffer absorption smooths out the
            // different input split points into uniform max_write-sized writes
            assert_eq!(
                &bw.inner.writes,
                &[&data[0..50], &data[50..100], &data[100..150]]
            );
            bw.flush().await.unwrap();
            assert_eq!(
                &bw.inner.writes,
                &[&data[0..50], &data[50..100], &data[100..150], &data[150..]]
            );
        });
    }

    #[test]
    fn buffer_is_exactly_full() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            bw.write_all(&data[..100]).await.unwrap();
            bw.write_all(&data[100..]).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.data(), &data);
        });
    }

    fn test_x<const SIZE: usize>(capacity: usize, max_write: Option<usize>, split: usize) {
        futures_lite::future::block_on(async {
            for _ in 0..100 {
                let data = rand_bytes::<SIZE>();
                let mut tw = TestWrite::new(max_write);
                let mut bw =
                    BufWriter::new_with_buffer(Vec::with_capacity(capacity), &mut tw, capacity);
                bw.write_all(&data[..split]).await.unwrap();
                bw.write_all(&data[split..]).await.unwrap();
                bw.flush().await.unwrap();
                assert_eq!(
                    &bw.inner.data(),
                    &data,
                    "test_x({},{:?},{split})",
                    bw.buffer.capacity(),
                    bw.inner.max_write
                );
            }
        });
    }

    #[test]
    fn known_bad() {
        test_x::<200>(188, Some(47), 123);
    }

    #[test]
    fn random() {
        for _ in 0..100 {
            test_x::<200>(
                fastrand::usize(1..200),
                Some(fastrand::usize(1..200)),
                fastrand::usize(1..200),
            );
        }
    }

    #[test]
    fn buffer_mut_after_body_streaming() {
        futures_lite::future::block_on(async {
            let body = rand_bytes::<500>();
            let trailer = b"trailer-content\r\n";
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(50), &mut tw, 1024);

            // Simulate body streaming
            bw.write_all(&body).await.unwrap();

            // Append trailers via buffer_mut — this was broken before
            bw.buffer_mut().extend_from_slice(trailer);

            // Flush should send trailers
            bw.flush().await.unwrap();

            let mut expected = body.to_vec();
            expected.extend_from_slice(trailer);
            assert_eq!(bw.inner.data(), expected);
        });
    }

    #[test]
    fn backpressure_absorption() {
        futures_lite::future::block_on(async {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(Some(30));
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(10), &mut tw, 200);
            // Write more than inner can accept at once — remainder should be absorbed
            bw.write_all(&data).await.unwrap();
            bw.flush().await.unwrap();
            assert_eq!(bw.inner.data(), data);
        });
    }

    #[test]
    fn write_vectored_basic() {
        futures_lite::future::block_on(async {
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            let a = b"hello ";
            let b = b"world";
            let bufs = [IoSlice::new(a), IoSlice::new(b)];
            let n = AsyncWriteExt::write_vectored(&mut bw, &bufs).await.unwrap();
            assert_eq!(n, 11);
            bw.flush().await.unwrap();
            assert_eq!(bw.inner.data(), b"hello world");
        });
    }

    #[test]
    fn write_vectored_with_pending_buffer() {
        futures_lite::future::block_on(async {
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::new(), &mut tw, 100);
            // Put some content in the buffer via buffer_mut
            bw.buffer_mut().extend_from_slice(b"header: value\r\n");

            let a = b"hello ";
            let b = b"world";
            let bufs = [IoSlice::new(a), IoSlice::new(b)];
            let n = AsyncWriteExt::write_vectored(&mut bw, &bufs).await.unwrap();
            assert_eq!(n, 11);
            bw.flush().await.unwrap();
            assert_eq!(bw.inner.data(), b"header: value\r\nhello world");
        });
    }

    #[test]
    fn write_vectored_partial_absorbs_remainder() {
        futures_lite::future::block_on(async {
            // inner can only write 4 bytes at a time
            let mut tw = TestWrite::new(Some(4));
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw, 100);
            let bufs = [
                IoSlice::new(b"aaa"),
                IoSlice::new(b"bbb"),
                IoSlice::new(b"ccc"),
            ];
            let n = AsyncWriteExt::write_vectored(&mut bw, &bufs).await.unwrap();
            // Reports full 9 bytes written despite inner only accepting 4
            assert_eq!(n, 9);
            // Inner got first 4 bytes
            assert_eq!(&bw.inner.writes, &[b"aaab"]);
            // Remaining 5 bytes absorbed into buffer
            bw.flush().await.unwrap();
            assert_eq!(bw.inner.data(), b"aaabbbccc");
        });
    }
}
