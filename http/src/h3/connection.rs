use super::{
    H3Error,
    frame::{Frame, FrameDecodeError, UniStreamType},
    quic_varint::{self, QuicVarIntError},
    settings::H3Settings,
};
use crate::{Buffer, Conn, ServerConfig, h3::H3ErrorCode};
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{
    future::Future,
    io::{self, ErrorKind},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
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
    server_config: Arc<ServerConfig>,

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
}

impl H3Connection {
    /// Construct a new `H3Connection` to manage HTTP/3 for a given peer.
    pub fn new(server_config: Arc<ServerConfig>) -> Arc<Self> {
        let swansong = server_config.swansong.child();
        Arc::new(Self {
            server_config,
            swansong,
            peer_settings: OnceLock::new(),
            max_accepted_stream_id: AtomicU64::new(0),
            has_accepted_stream: AtomicBool::new(false),
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
    /// [`ServerConfig::shut_down`]
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// Retrieve the [`ServerConfig`] for this server.
    pub fn server_config(&self) -> Arc<ServerConfig> {
        self.server_config.clone()
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
        let buffer =
            Vec::with_capacity(self.server_config.http_config.request_buffer_initial_len).into();
        match Conn::new_h3(self, transport, buffer).await? {
            H3StreamResult::Request(conn) => Ok(H3StreamResult::Request(
                handler(conn).await.send_h3().await?,
            )),
            wt @ H3StreamResult::WebTransport { .. } => Ok(wt),
        }
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
        let settings = Frame::Settings(H3Settings::from(&self.server_config.http_config));

        write(&mut buf, &mut stream, |buf| {
            let mut written = quic_varint::encode(UniStreamType::Control, buf)?;
            written += settings.encode(&mut buf[written..])?;
            Some(written)
        })
        .await?;

        // Wait for shutdown
        self.swansong.clone().await;

        // Send GOAWAY
        write(&mut buf, &mut stream, |buf| {
            Frame::Goaway(self.goaway_id()).encode(buf)
        })
        .await?;

        Ok(())
    }

    /// Initialize and hold open the outbound QPACK encoder stream for the duration of the
    /// connection.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    // Currently idle (static table only); will carry encoder instructions when dynamic table is
    // added.
    pub async fn run_encoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut buf = vec![0; 8];
        write(&mut buf, &mut stream, |buf| {
            quic_varint::encode(UniStreamType::QpackEncoder, buf)
        })
        .await?;

        self.swansong.clone().await;
        Ok(())
    }

    /// Initialize and hold open the outbound QPACK decoder stream for the duration of the
    /// connection.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` in case of io error or http/3 semantic error.
    // Currently idle (static table only); will carry decoder instructions when dynamic table is
    // added.
    pub async fn run_decoder<T>(&self, mut stream: T) -> Result<(), H3Error>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let mut buf = vec![0; 8];
        write(&mut buf, &mut stream, |buf| {
            quic_varint::encode(UniStreamType::QpackDecoder, buf)
        })
        .await?;

        self.swansong.clone().await;
        Ok(())
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
                self.run_inbound_control(&mut buf, &mut filled, &mut stream)
                    .await?;
                Ok(UniStreamResult::Handled)
            }

            Ok(UniStreamType::QpackEncoder | UniStreamType::QpackDecoder) => {
                // Static table only — hold stream open until shutdown
                self.swansong.clone().await;
                Ok(UniStreamResult::Handled)
            }

            Ok(UniStreamType::WebTransport) => {
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

            Ok(UniStreamType::Push) | Err(_) => Ok(UniStreamResult::Unknown {
                stream_type,
                stream,
            }),
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

        // Read subsequent frames, watching for GOAWAY
        loop {
            let frame = read(buf, filled, stream, |data| match Frame::decode(data) {
                Ok((frame, consumed)) => Ok(Some((frame, consumed))),
                Err(FrameDecodeError::Incomplete) => Ok(None),
                Err(FrameDecodeError::Error(code)) => Err(code),
            })
            .await?;

            match frame {
                Frame::Goaway(_) => {
                    self.swansong.shut_down();
                    return Ok(());
                }
                Frame::Settings(_) => {
                    return Err(H3ErrorCode::FrameUnexpected.into());
                }

                _ => { /* MAX_PUSH_ID, CANCEL_PUSH, unknown — ignored for now */ }
            }
        }
    }
}

const MAX_BUFFER_SIZE: usize = 1024 * 10;

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
