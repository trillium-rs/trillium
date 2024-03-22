use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    fmt,
    io::{Error, ErrorKind, IoSlice, Result},
    pin::Pin,
    task::{ready, Context, Poll},
};
use trillium_macros::AsyncRead;

#[derive(AsyncRead)]
pub(crate) struct BufWriter<W> {
    #[async_read]
    inner: W,
    buffer: Vec<u8>,
    written_to_inner: usize,
}

impl<W: AsyncWrite + Unpin> BufWriter<W> {
    pub(crate) fn new_with_buffer(buffer: Vec<u8>, inner: W) -> Self {
        Self {
            inner,
            buffer,
            written_to_inner: 0,
        }
    }

    fn poll_flush_buf(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<usize>> {
        let Self {
            inner,
            buffer,
            written_to_inner,
        } = &mut *self;

        let len = buffer.len();
        let mut ret = Ok(0);

        while *written_to_inner < len {
            let buf = &buffer[*written_to_inner..];
            match ready!(Pin::new(&mut *inner).poll_write(cx, buf)) {
                Ok(0) => {
                    ret = Err(Error::new(
                        ErrorKind::WriteZero,
                        "Failed to write buffered data",
                    ));
                    break;
                }
                Ok(n) => *written_to_inner += n,
                Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    ret = Err(e);
                    break;
                }
            }
        }

        Poll::Ready(ret)
    }
}

impl<W: fmt::Debug> fmt::Debug for BufWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufWriter")
            .field("writer", &self.inner)
            .field("buf", &self.buffer)
            .field("written", &self.written_to_inner)
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
            written_to_inner,
        } = &mut *self;
        loop {
            let len = buffer.len();
            let pending_buffer = &buffer[len.min(*written_to_inner)..];
            let pending_bytes = pending_buffer.len();
            let new_bytes = additional.len();
            let new_len_would_be = len + new_bytes;
            if *written_to_inner == 0 && new_len_would_be <= buffer.capacity() {
                buffer.extend_from_slice(additional);
                return Poll::Ready(Ok(additional.len()));
            } else if !pending_buffer.is_empty() {
                let written = ready!(Pin::new(&mut *inner).poll_write_vectored(
                    cx,
                    &[IoSlice::new(pending_buffer), IoSlice::new(additional)]
                ))?;
                *written_to_inner += written;
                let written_from_additional = written.saturating_sub(pending_bytes);
                if written_from_additional != 0 {
                    return Poll::Ready(Ok(written_from_additional));
                }
            } else {
                let written = ready!(Pin::new(&mut *inner).poll_write(cx, additional))?;
                *written_to_inner += written;
                return Poll::Ready(Ok(written));
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        Pin::new(&mut self.inner).poll_close(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        additional_bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        let Self {
            inner,
            buffer,
            written_to_inner,
        } = &mut *self;

        loop {
            let len = buffer.len();
            let pending_buffer = &buffer[len.min(*written_to_inner)..];
            let pending_bytes = pending_buffer.len();
            let new_bytes = additional_bufs.iter().map(|x| x.len()).sum();
            if *written_to_inner == 0 && (len + new_bytes) <= buffer.capacity() {
                buffer.reserve(new_bytes);
                for additional in additional_bufs {
                    buffer.extend_from_slice(additional);
                }
                return Poll::Ready(Ok(new_bytes));
            } else if pending_buffer.is_empty() {
                let written =
                    ready!(Pin::new(&mut *inner).poll_write_vectored(cx, additional_bufs))?;
                *written_to_inner += written;
                return Poll::Ready(Ok(written));
            } else {
                let mut vectored = Vec::with_capacity(additional_bufs.len() + 1);
                vectored.push(IoSlice::new(pending_buffer));
                vectored.extend_from_slice(additional_bufs);
                let written = ready!(Pin::new(&mut *inner).poll_write_vectored(cx, &vectored))?;
                *written_to_inner += written;
                let written_from_additional = written.saturating_sub(pending_bytes);
                if written_from_additional != 0 {
                    return Poll::Ready(Ok(written_from_additional));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::AsyncWriteExt;
    use pretty_assertions::assert_eq;
    use test_harness::test;
    use trillium_testing::harness;
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

    mod write {
        use super::{test, *};

        #[test(harness)]
        async fn entire_content_shorter_than_capacity() {
            let data = rand_bytes::<90>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw);
            bw.write_all(&data).await.unwrap();
            assert_eq!(bw.inner.writes.len(), 0);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
        }

        #[test(harness)]
        async fn longer_than_capacity_but_still_a_single_write() {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw);
            bw.write_all(&data).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
        }

        #[test(harness)]
        async fn multiple_writes() {
            let data = rand_bytes::<250>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw);
            bw.write_all(&data[..200]).await.unwrap();
            bw.write_all(&data[200..]).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data[..200], &data[200..]]);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data[..200], &data[200..]]);
        }

        #[test(harness)]
        async fn overflow_is_vectored() {
            let data = rand_bytes::<101>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw);
            bw.write_all(&data[..50]).await.unwrap();
            bw.write_all(&data[50..]).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
        }

        #[test(harness)]
        fn max_write() {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(Some(50));
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw);
            bw.write_all(&data[..10]).await.unwrap();
            bw.write_all(&data[10..20]).await.unwrap();
            bw.write_all(&data[20..45]).await.unwrap();
            bw.write_all(&data[45..125]).await.unwrap();
            bw.write_all(&data[125..]).await.unwrap();
            for write in &bw.inner.writes {
                println!(
                    "{}",
                    write
                        .iter()
                        .map(u8::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            assert_eq!(
                &bw.inner.writes,
                &[
                    &data[0..50],
                    &data[50..100],
                    &data[100..125],
                    &data[125..175],
                    &data[175..]
                ]
            );
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.data(), &data);
        }

        #[test(harness)]
        fn write_boundary_is_exactly_buffer_len() {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(Some(50));
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw);
            bw.write_all(&data[..10]).await.unwrap();
            bw.write_all(&data[10..20]).await.unwrap();
            bw.write_all(&data[20..50]).await.unwrap();
            bw.write_all(&data[50..125]).await.unwrap();
            bw.write_all(&data[125..]).await.unwrap();
            assert_eq!(
                &bw.inner.writes,
                &[
                    &data[0..50],
                    &data[50..100],
                    &data[100..125],
                    &data[125..175],
                    &data[175..]
                ]
            );
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.data(), &data);
        }

        #[test(harness)]
        fn buffer_is_exactly_full() {
            let data = rand_bytes::<200>();
            let mut tw = TestWrite::new(None);
            let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(100), &mut tw);
            bw.write_all(&data[..100]).await.unwrap();
            bw.write_all(&data[100..]).await.unwrap();
            assert_eq!(&bw.inner.writes, &[&data]);
            bw.flush().await.unwrap();
            assert_eq!(&bw.inner.data(), &data);
        }

        async fn test_x<const SIZE: usize>(
            capacity: usize,
            max_write: Option<usize>,
            split: usize,
        ) {
            for _ in 0..100 {
                let data = rand_bytes::<SIZE>();
                let mut tw = TestWrite::new(max_write);
                let mut bw = BufWriter::new_with_buffer(Vec::with_capacity(capacity), &mut tw);
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
        }

        #[test(harness)]
        async fn known_bad() {
            test_x::<200>(188, Some(47), 123).await;
        }

        #[test(harness)]
        async fn random() {
            for _ in 0..100 {
                test_x::<200>(
                    fastrand::usize(1..200),
                    Some(fastrand::usize(1..200)),
                    fastrand::usize(1..200),
                );
            }
        }
    }
}
