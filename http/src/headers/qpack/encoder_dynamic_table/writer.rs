//! Writes this connection's QPACK encoder stream (RFC 9204 §4.2).
//!
//! Runs as a connection-scoped task. Sends the stream type byte, then loops draining
//! already-encoded instructions from the [`EncoderDynamicTable`] op queue, writing each
//! batch to the underlying unidirectional stream. Returns when the swansong resolves or
//! the table is marked failed.

use super::EncoderDynamicTable;
use crate::h3::{H3Error, UniStreamType, quic_varint};
use futures_lite::io::{AsyncWrite, AsyncWriteExt};
use swansong::Swansong;

impl EncoderDynamicTable {
    /// Run the encoder-stream writer loop for the duration of the connection.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` on I/O failure.
    pub(crate) async fn run_writer<T>(
        &self,
        stream: &mut T,
        swansong: Swansong,
    ) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        log::trace!("QPACK encoder stream writer: opening");
        let mut type_buf = [0u8; 8];
        let n = quic_varint::encode(UniStreamType::QpackEncoder, &mut type_buf)
            .expect("stream type varint fits in 8 bytes");
        stream.write_all(&type_buf[..n]).await?;
        stream.flush().await?;
        log::trace!("QPACK encoder stream writer: type byte sent");

        loop {
            let listener = self.listen();

            if let Some(code) = self.failed() {
                log::debug!("QPACK encoder stream writer: table failed ({code}), exiting");
                return Ok(());
            }

            let ops = self.drain_pending_ops();
            if !ops.is_empty() {
                let total: usize = ops.iter().map(Vec::len).sum();
                log::trace!(
                    "QPACK encoder stream writer: flushing {} ops ({total} bytes)",
                    ops.len()
                );
                for op in &ops {
                    stream.write_all(op).await?;
                }
                stream.flush().await?;
            }

            let shutdown = futures_lite::future::or(
                async {
                    listener.await;
                    false
                },
                async {
                    swansong.clone().await;
                    true
                },
            )
            .await;

            if shutdown {
                log::trace!("QPACK encoder stream writer: shutdown requested");
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        HeaderName, HeaderValue,
        h3::{H3ErrorCode, UniStreamType},
        headers::qpack::decoder_dynamic_table::DecoderDynamicTable,
    };
    use futures_lite::{
        AsyncRead,
        future::{block_on, or},
        io::AsyncReadExt,
    };
    use std::{
        io,
        pin::Pin,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll, Waker},
    };
    use swansong::Swansong;

    /// An in-memory duplex: a writer feeds bytes into a shared buffer that a reader consumes.
    /// Used to test `run_encoder_stream_writer` by feeding its output into
    /// `process_encoder_stream` running against a decoder-side `DecoderDynamicTable`.
    #[derive(Clone)]
    struct Duplex {
        inner: Arc<Mutex<DuplexInner>>,
    }

    struct DuplexInner {
        buf: Vec<u8>,
        pos: usize,
        reader_waker: Option<Waker>,
        closed: bool,
    }

    impl Duplex {
        fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(DuplexInner {
                    buf: Vec::new(),
                    pos: 0,
                    reader_waker: None,
                    closed: false,
                })),
            }
        }

        fn close(&self) {
            let mut inner = self.inner.lock().unwrap();
            inner.closed = true;
            if let Some(waker) = inner.reader_waker.take() {
                waker.wake();
            }
        }
    }

    impl AsyncWrite for Duplex {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut inner = self.inner.lock().unwrap();
            inner.buf.extend_from_slice(buf);
            if let Some(waker) = inner.reader_waker.take() {
                waker.wake();
            }
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Duplex::close(&self);
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncRead for Duplex {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            let mut inner = self.inner.lock().unwrap();
            let available = inner.buf.len() - inner.pos;
            if available > 0 {
                let n = available.min(buf.len());
                buf[..n].copy_from_slice(&inner.buf[inner.pos..inner.pos + n]);
                inner.pos += n;
                Poll::Ready(Ok(n))
            } else if inner.closed {
                Poll::Ready(Ok(0))
            } else {
                inner.reader_waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from(s.as_bytes().to_vec())
    }

    fn hn(s: &str) -> HeaderName<'static> {
        HeaderName::parse(s.as_bytes()).unwrap().into_owned()
    }

    #[test]
    fn writes_stream_type_and_drains_ops() {
        let table = Arc::new(EncoderDynamicTable::new(4096));
        table.enqueue_set_capacity(4096).unwrap();
        table
            .enqueue_insert_literal(hn("x-custom"), hv("v"))
            .unwrap();

        let swansong = Swansong::new();
        let duplex = Duplex::new();

        let writer_done = Arc::new(AtomicBool::new(false));
        let writer_done_clone = writer_done.clone();
        let table_clone = table.clone();
        let swansong_clone = swansong.clone();
        let mut duplex_clone = duplex.clone();
        let writer_task = async move {
            table_clone
                .run_writer(&mut duplex_clone, swansong_clone)
                .await
                .unwrap();
            writer_done_clone.store(true, Ordering::SeqCst);
        };

        let reader_task = async {
            // Read the stream type varint (single byte for QpackEncoder).
            let mut stream_type_byte = [0u8; 1];
            let mut stream = duplex.clone();
            stream.read_exact(&mut stream_type_byte).await.unwrap();
            assert_eq!(stream_type_byte[0], UniStreamType::QpackEncoder as u8);

            // Feed the rest into process_encoder_stream against a decoder table.
            let decoder_table = DecoderDynamicTable::new(4096, 0);
            // We have exactly two instructions queued; once they're consumed, closing the
            // duplex lets process_encoder_stream see EOF and return Ok.
            let processed = async {
                decoder_table.run_reader(&mut stream).await.unwrap();
            };

            // Wait long enough for the writer to emit both ops, then close so the reader exits.
            let closer = async {
                // Spin on the duplex buffer until the expected byte count arrives.
                loop {
                    let filled = duplex.inner.lock().unwrap().buf.len();
                    if filled >= 4 {
                        break;
                    }
                    futures_lite::future::yield_now().await;
                }
                // Give the encoder-stream loop a chance to fully consume the instructions,
                // then request shutdown and close the stream so the reader unblocks.
                for _ in 0..100 {
                    futures_lite::future::yield_now().await;
                }
                swansong.shut_down().await;
                duplex.close();
            };

            futures_lite::future::zip(processed, closer).await;
            assert_eq!(
                decoder_table.name_at_relative(0).unwrap().as_ref(),
                "x-custom"
            );
        };

        block_on(futures_lite::future::zip(writer_task, reader_task));
        assert!(writer_done.load(Ordering::SeqCst));
    }

    #[test]
    fn exits_on_swansong_with_no_ops() {
        let table = Arc::new(EncoderDynamicTable::new(4096));
        let swansong = Swansong::new();
        let duplex = Duplex::new();

        let mut duplex_clone = duplex.clone();
        let writer_task = table.run_writer(&mut duplex_clone, swansong.clone());
        let shutdown_task = async {
            // Give the writer a tick to send its type byte and enter the wait.
            for _ in 0..10 {
                futures_lite::future::yield_now().await;
            }
            swansong.shut_down().await;
        };

        block_on(or(
            async {
                writer_task.await.unwrap();
            },
            shutdown_task,
        ));

        // The stream type byte should have been written.
        assert!(!duplex.inner.lock().unwrap().buf.is_empty());
    }

    #[test]
    fn exits_on_table_failure() {
        let table = Arc::new(EncoderDynamicTable::new(4096));
        let swansong = Swansong::new();
        let mut duplex = Duplex::new();

        let table_clone = table.clone();
        let trigger = async move {
            for _ in 0..10 {
                futures_lite::future::yield_now().await;
            }
            table_clone.fail(H3ErrorCode::QpackDecoderStreamError);
        };

        let writer = table.run_writer(&mut duplex, swansong);
        let (result, ()) = block_on(futures_lite::future::zip(writer, trigger));
        result.unwrap();
    }
}
