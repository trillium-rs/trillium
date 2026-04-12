use super::{
    H3Error,
    frame::{Frame, FrameDecodeError, UniStreamType},
    quic_varint::{self, QuicVarIntError},
    settings::H3Settings,
};
use crate::{
    Buffer, Conn, HttpContext,
    h3::{H3ErrorCode, MAX_BUFFER_SIZE},
    headers::qpack::{DecoderDynamicTable, EncoderDynamicTable, FieldSection},
};
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{
    future::Future,
    io::{self, ErrorKind},
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
};
use swansong::{ShutdownCompletion, Swansong};

/// The result of processing an HTTP/3 bidirectional stream.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // Request is the hot path; boxing it would add an allocation per request
pub enum H3StreamResult<Transport> {
    /// The stream carried a normal HTTP/3 request.
    Request(Conn<Transport>),

    /// The stream carries a WebTransport bidirectional data stream. The `session_id` identifies
    /// the associated WebTransport session.
    WebTransport {
        /// The WebTransport session ID (stream ID of the CONNECT request).
        session_id: u64,
        /// The underlying transport, ready for application data.
        transport: Transport,
        /// Any bytes buffered after the session ID during stream negotiation.
        buffer: Buffer,
    },
}

/// The result of processing an HTTP/3 unidirectional stream.
#[derive(Debug)]
pub enum UniStreamResult<T> {
    /// The stream was a known internal type (control, QPACK encoder/decoder) and was handled
    /// automatically.
    Handled,

    /// A WebTransport unidirectional data stream. The `session_id` identifies the associated
    /// WebTransport session.
    WebTransport {
        /// The WebTransport session ID.
        session_id: u64,
        /// The receive stream, ready for application data.
        stream: T,
        /// Any bytes buffered after the session ID during stream negotiation.
        buffer: Buffer,
    },

    /// An unknown or unsupported stream type (e.g. Push). The caller should close or reset
    /// this stream without processing it.
    Unknown {
        /// The raw stream type value.
        stream_type: u64,
        /// The stream.
        stream: T,
    },
}

/// Shared state for a single HTTP/3 QUIC connection.
///
/// Call the appropriate methods on this type for each stream accepted from the QUIC connection.
#[derive(Debug)]
pub struct H3Connection {
    /// Shared configuration for the entire server, including tcp-based listeners
    context: Arc<HttpContext>,

    /// Connection-scoped shutdown signal. Shut down when we receive GOAWAY from the peer or when
    /// the server-level Swansong shuts down.  Request stream tasks use this to interrupt
    /// in-progress work.
    swansong: Swansong,

    /// The peer's H3 settings, received on their control stream.  Request streams may need to
    /// consult these (e.g. max field section size).
    peer_settings: OnceLock<H3Settings>,

    /// The highest bidirectional stream ID we have accepted.  Used to compute the GOAWAY value
    /// (this + 4) to tell the peer which requests we saw. None until the first stream is accepted.
    /// Updated by the runtime adapter's accept loop via [`record_accepted_stream`].
    max_accepted_stream_id: AtomicU64,

    /// Whether we have accepted any streams yet.
    has_accepted_stream: AtomicBool,

    /// The decoder-side QPACK dynamic table for this connection.
    decoder_dynamic_table: DecoderDynamicTable,

    /// The encoder-side QPACK dynamic table for this connection.
    encoder_dynamic_table: EncoderDynamicTable,
}

impl H3Connection {
    /// Construct a new `H3Connection` to manage HTTP/3 for a given peer.
    pub fn new(context: Arc<HttpContext>) -> Arc<Self> {
        let swansong = context.swansong.child();
        let max_table_capacity = context.config.h3_max_table_capacity;
        let blocked_streams = context.config.h3_blocked_streams;
        Arc::new(Self {
            context,
            swansong,
            peer_settings: OnceLock::new(),
            max_accepted_stream_id: AtomicU64::new(0),
            has_accepted_stream: AtomicBool::new(false),
            decoder_dynamic_table: DecoderDynamicTable::new(max_table_capacity, blocked_streams),
            encoder_dynamic_table: EncoderDynamicTable::new(max_table_capacity),
        })
    }

    /// Retrieve the [`Swansong`] shutdown handle for this HTTP/3 connection. See also
    /// [`H3Connection::shut_down`]
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Attempt graceful shutdown of this HTTP/3 connection (all streams).
    ///
    /// The returned [`ShutdownCompletion`] type can
    /// either be awaited in an async context or blocked on with [`ShutdownCompletion::block`] in a
    /// blocking context
    ///
    /// Note that this will NOT shut down the server. To shut down the whole server, use
    /// [`HttpContext::shut_down`]
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// Retrieve the [`HttpContext`] for this server.
    pub fn context(&self) -> Arc<HttpContext> {
        self.context.clone()
    }

    /// Returns the peer's HTTP/3 settings, available once the peer's control stream has been
    /// processed.
    pub fn peer_settings(&self) -> Option<&H3Settings> {
        self.peer_settings.get()
    }

    /// Record that we accepted a bidirectional stream with this ID.
    fn record_accepted_stream(&self, stream_id: u64) {
        self.max_accepted_stream_id
            .fetch_max(stream_id, Ordering::Relaxed);
        self.has_accepted_stream.store(true, Ordering::Relaxed);
    }

    /// The stream ID to send in a GOAWAY frame: one past the highest stream we accepted, or 0 if we
    /// haven't accepted any.
    fn goaway_id(&self) -> u64 {
        if self.has_accepted_stream.load(Ordering::Relaxed) {
            self.max_accepted_stream_id.load(Ordering::Relaxed) + 4
        } else {
            0
        }
    }

    /// Process a single HTTP/3 request-response cycle on a bidirectional stream.
    ///
    /// Call this once per accepted bidirectional stream. Returns
    /// [`H3StreamResult::WebTransport`] if the stream opens a WebTransport session rather than
    /// a standard HTTP/3 request.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    pub async fn process_inbound_bidi<Transport, Handler, Fut>(
        self: Arc<Self>,
        transport: Transport,
        handler: Handler,
        stream_id: u64,
    ) -> Result<H3StreamResult<Transport>, H3Error>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnOnce(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        self.record_accepted_stream(stream_id);
        let _guard = self.swansong.guard();
        let buffer = Vec::with_capacity(self.context.config.request_buffer_initial_len).into();
        match Conn::new_h3(self, transport, buffer, stream_id).await? {
            H3StreamResult::Request(conn) => Ok(H3StreamResult::Request(
                handler(conn).await.send_h3().await?,
            )),
            wt @ H3StreamResult::WebTransport { .. } => Ok(wt),
        }
    }

    /// Decode a QPACK-encoded field section, consulting the dynamic table as needed.
    ///
    /// If the field section's Required Insert Count is greater than zero, waits until the
    /// dynamic table has received enough entries. Returns an error on protocol violations or
    /// if the encoder stream fails while waiting.
    ///
    /// Duplicate pseudo-headers are silently ignored (first value wins).
    /// Unknown pseudo-headers are rejected per RFC 9114 §4.1.1.
    ///
    /// # Errors
    ///
    /// Returns an error if the encoded bytes cannot be parsed as a valid field section.
    #[cfg(feature = "unstable")]
    pub async fn decode_field_section(
        &self,
        encoded: &[u8],
        stream_id: u64,
    ) -> Result<FieldSection<'static>, H3Error> {
        FieldSection::decode_with_dynamic_table(encoded, &self.decoder_dynamic_table, stream_id)
            .await
    }

    #[cfg(not(feature = "unstable"))]
    pub(crate) async fn decode_field_section(
        &self,
        encoded: &[u8],
        stream_id: u64,
    ) -> Result<FieldSection<'static>, H3Error> {
        FieldSection::decode_with_dynamic_table(encoded, &self.decoder_dynamic_table, stream_id)
            .await
    }

    /// Encode a QPACK field section from pseudo-headers and headers.
    ///
    /// This currently uses only the static table (no dynamic table).
    /// Decode a QPACK-encoded field section, consulting the dynamic table as needed.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of http/3 semantic error.
    #[cfg(feature = "unstable")]
    pub async fn encode_field_section(
        &self,
        field_section: &FieldSection<'_>,
        buf: &mut Vec<u8>,
        stream_id: u64,
    ) -> Result<(), H3Error> {
        std::future::ready(()).await;
        let _ = stream_id;
        field_section.encode(buf);
        Ok(())
    }

    #[cfg(not(feature = "unstable"))]
    pub(crate) async fn encode_field_section(
        &self,
        field_section: &FieldSection<'_>,
        buf: &mut Vec<u8>,
        stream_id: u64,
    ) -> Result<(), H3Error> {
        std::future::ready(()).await;
        let _ = stream_id;
        field_section.encode(buf);
        Ok(())
    }

    /// Run this server's HTTP/3 outbound control stream.
    ///
    /// Sends the initial SETTINGS frame, then sends GOAWAY when the connection shuts down.
    /// Returns after GOAWAY is sent; keep the stream open until the QUIC connection closes
    /// (closing a control stream is a connection error per RFC 9114 §6.2.1).
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    pub async fn run_outbound_control<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut buf = vec![0; 128];

        // Stream type + SETTINGS frame
        let settings = Frame::Settings(H3Settings::from(&self.context.config));
        log::trace!(
            "H3 outbound control: sending SETTINGS: {:?}",
            H3Settings::from(&self.context.config)
        );

        write(&mut buf, &mut stream, |buf| {
            let mut written = quic_varint::encode(UniStreamType::Control, buf)?;
            written += settings.encode(&mut buf[written..])?;
            Some(written)
        })
        .await?;
        log::trace!("H3 outbound control: SETTINGS sent");

        // Wait for shutdown
        self.swansong.clone().await;

        // Send GOAWAY
        write(&mut buf, &mut stream, |buf| {
            Frame::Goaway(self.goaway_id()).encode(buf)
        })
        .await?;

        Ok(())
    }

    /// Run the outbound QPACK encoder stream for the duration of the connection.
    ///
    /// Writes the stream type byte, then drains encoder-stream instructions from the encoder
    /// dynamic table as they are enqueued. Returns when the connection shuts down or the table is
    /// marked failed.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error.
    pub async fn run_encoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.encoder_dynamic_table
            .run_writer(&mut stream, self.swansong.clone())
            .await
    }

    /// Run the outbound QPACK decoder stream for the duration of the connection.
    ///
    /// Writes the stream type byte, then loops sending Section Acknowledgement and Insert
    /// Count Increment instructions as they become needed. Returns when the connection
    /// shuts down.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    pub async fn run_decoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.decoder_dynamic_table
            .run_writer(&mut stream, self.swansong.clone())
            .await
    }

    /// Handle an inbound unidirectional HTTP/3 stream from the peer.
    ///
    /// Internal stream types (control, QPACK encoder/decoder) are handled automatically;
    /// application streams are returned via [`UniStreamResult`] for the caller to process.
    ///
    /// # Errors
    ///
    /// Returns a `H3Error` in case of io error or http/3 semantic error.
    pub async fn process_inbound_uni<T>(&self, mut stream: T) -> Result<UniStreamResult<T>, H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut buf = vec![0; 128];
        let mut filled = 0;

        // Read stream type varint (decode as raw u64 to handle unknown types)
        let stream_type = read(
            &mut buf,
            &mut filled,
            &mut stream,
            |data| match quic_varint::decode::<u64>(data) {
                Ok(ok) => Ok(Some(ok)),
                Err(QuicVarIntError::UnexpectedEnd) => Ok(None),
                // this branch is unreachable because u64 is always From<u64>
                Err(QuicVarIntError::UnknownValue { bytes, value }) => Ok(Some((value, bytes))),
            },
        )
        .await?;

        match UniStreamType::try_from(stream_type) {
            Ok(UniStreamType::Control) => {
                log::trace!("H3 inbound uni: control stream");
                self.run_inbound_control(&mut buf, &mut filled, &mut stream)
                    .await?;
                Ok(UniStreamResult::Handled)
            }

            Ok(UniStreamType::QpackEncoder) => {
                log::trace!("H3 inbound uni: QPACK encoder stream ({filled} bytes pre-read)");
                let mut reader = Prepended {
                    head: &buf[..filled],
                    tail: stream,
                };

                log::trace!("QPACK encoder stream: started");
                self.decoder_dynamic_table.run_reader(&mut reader).await?;

                Ok(UniStreamResult::Handled)
            }

            Ok(UniStreamType::QpackDecoder) => {
                log::trace!("H3 inbound uni: QPACK decoder stream ({filled} bytes pre-read)");
                let mut reader = Prepended {
                    head: &buf[..filled],
                    tail: stream,
                };
                self.encoder_dynamic_table.run_reader(&mut reader).await?;
                Ok(UniStreamResult::Handled)
            }

            Ok(UniStreamType::WebTransport) => {
                log::trace!("H3 inbound uni: WebTransport stream");
                let session_id =
                    read(
                        &mut buf,
                        &mut filled,
                        &mut stream,
                        |data| match quic_varint::decode::<u64>(data) {
                            Ok(ok) => Ok(Some(ok)),
                            Err(QuicVarIntError::UnexpectedEnd) => Ok(None),
                            Err(QuicVarIntError::UnknownValue { bytes, value }) => {
                                Ok(Some((value, bytes)))
                            }
                        },
                    )
                    .await?;
                buf.truncate(filled);
                Ok(UniStreamResult::WebTransport {
                    session_id,
                    stream,
                    buffer: buf.into(),
                })
            }

            Ok(UniStreamType::Push) | Err(_) => {
                log::trace!("H3 inbound uni: unknown stream type {stream_type:#x}");
                Ok(UniStreamResult::Unknown {
                    stream_type,
                    stream,
                })
            }
        }
    }

    /// Handle the http/3 peer's inbound control stream.
    ///
    /// # Errors
    ///
    /// Returns a `H3Error` in case of io error or HTTP/3 semantic error.
    // The first frame must be SETTINGS. After that, watches for
    // GOAWAY to initiate connection shutdown.
    async fn run_inbound_control<T>(
        &self,
        buf: &mut Vec<u8>,
        filled: &mut usize,
        stream: &mut T,
    ) -> Result<(), H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        // First frame must be SETTINGS (§6.2.1)
        let settings = read(buf, filled, stream, |data| match Frame::decode(data) {
            Ok((Frame::Settings(s), consumed)) => Ok(Some((s, consumed))),
            Ok(_) => Err(H3ErrorCode::FrameUnexpected),
            Err(FrameDecodeError::Incomplete) => Ok(None),
            Err(FrameDecodeError::Error(code)) => Err(code),
        })
        .await?;

        self.peer_settings
            .set(settings)
            .map_err(|_| H3ErrorCode::FrameUnexpected)?;
        log::trace!("H3 peer settings: {:?}", self.peer_settings.get());

        // Read subsequent frames, watching for GOAWAY
        loop {
            let frame = read(buf, filled, stream, |data| match Frame::decode(data) {
                Ok((frame, consumed)) => Ok(Some((frame, consumed))),
                Err(FrameDecodeError::Incomplete) => Ok(None),
                Err(FrameDecodeError::Error(code)) => Err(code),
            })
            .await?;

            match frame {
                Frame::Goaway(id) => {
                    log::trace!("H3 control stream: peer sent GOAWAY(stream_id={id})");
                    self.swansong.shut_down();
                    return Ok(());
                }
                Frame::Settings(_) => {
                    return Err(H3ErrorCode::FrameUnexpected.into());
                }
                Frame::Unknown(n) => {
                    // RFC 9114 §7.2.8: unknown frame types MUST be ignored.
                    // We must also consume the payload bytes so the stream stays synchronized.
                    log::trace!("H3 control stream: skipping unknown frame (payload {n} bytes)");
                    let n = usize::try_from(n).unwrap_or(usize::MAX);
                    let in_buf = n.min(*filled);
                    buf.copy_within(in_buf..*filled, 0);
                    *filled -= in_buf;
                    let mut todo = n - in_buf;
                    let mut scratch = [0u8; 256];
                    while todo > 0 {
                        let to_read = todo.min(scratch.len());
                        let n = stream
                            .read(&mut scratch[..to_read])
                            .await
                            .map_err(H3Error::Io)?;
                        if n == 0 {
                            return Err(H3ErrorCode::ClosedCriticalStream.into());
                        }
                        todo -= n;
                    }
                }
                other => {
                    log::trace!("H3 control stream: ignoring {other:?}");
                }
            }
        }
    }
}

async fn write(
    buf: &mut Vec<u8>,
    mut stream: impl AsyncWrite + Unpin + Send,
    mut f: impl FnMut(&mut [u8]) -> Option<usize>,
) -> io::Result<usize> {
    let written = loop {
        if let Some(w) = f(buf) {
            break w;
        }
        if buf.len() >= MAX_BUFFER_SIZE {
            return Err(io::Error::new(ErrorKind::OutOfMemory, "runaway allocation"));
        }
        buf.resize(buf.len() * 2, 0);
    };

    stream.write_all(&buf[..written]).await?;
    stream.flush().await?;
    Ok(written)
}

/// An `AsyncRead` adapter that drains a byte slice before reading from an inner stream.
///
/// Used in `process_inbound_uni` to replay bytes that were read ahead while
/// parsing the stream-type varint before dispatching to `run_inbound_encoder`.
struct Prepended<'a, T> {
    head: &'a [u8],
    tail: T,
}

impl<T: AsyncRead + Unpin> AsyncRead for Prepended<'_, T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if !this.head.is_empty() {
            let n = this.head.len().min(out.len());
            out[..n].copy_from_slice(&this.head[..n]);
            this.head = &this.head[n..];
            return Poll::Ready(Ok(n));
        }
        Pin::new(&mut this.tail).poll_read(cx, out)
    }
}

/// Read from `stream` into `buf` until `f` can decode a value.
///
/// `f` receives the filled portion of the buffer and returns:
/// - `Ok(Some((value, consumed)))` — success; consumed bytes are removed from the front
/// - `Ok(None)` — need more data; reads more bytes and retries
/// - `Err(e)` — unrecoverable error; propagated to caller
async fn read<R>(
    buf: &mut Vec<u8>,
    filled: &mut usize,
    stream: &mut (impl AsyncRead + Unpin + Send),
    f: impl Fn(&[u8]) -> Result<Option<(R, usize)>, H3ErrorCode>,
) -> Result<R, H3Error> {
    loop {
        if let Some((result, consumed)) = f(&buf[..*filled])? {
            buf.copy_within(consumed..*filled, 0);
            *filled -= consumed;
            return Ok(result);
        }

        if *filled >= buf.len() {
            if buf.len() >= MAX_BUFFER_SIZE {
                return Err(io::Error::new(ErrorKind::OutOfMemory, "runaway allocation").into());
            }
            buf.resize(buf.len() * 2, 0);
        }

        let n = stream.read(&mut buf[*filled..]).await?;
        if n == 0 {
            return Err(io::Error::new(ErrorKind::UnexpectedEof, "stream closed").into());
        }
        *filled += n;
    }
}
