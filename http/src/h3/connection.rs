use super::{
    H3RequestError,
    frame::{Frame, FrameDecodeError, UniStreamType},
    quic_varint::{self, QuicVarIntError},
    settings::H3Settings,
};
use crate::{Conn, ServerConfig, h3::ErrorCode};
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

/// Per-QUIC-connection state, shared across all stream tasks via `Arc`.
///
/// The runtime adapter is responsible for accepting streams from the QUIC connection and spawning
/// tasks that call the appropriate method on this type.
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

    /// Retrieve the [`Swansong`] concurrency controller. See also [`shut_down`]
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Attempt graceful shutdown of this http3 connection (all streams).
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

    /// Retrieve the [`ServerConfig`] that is shared throughout this trillium server, including tcp
    /// listeners
    pub fn server_config(&self) -> Arc<ServerConfig> {
        self.server_config.clone()
    }

    /// Retrieve this http/3 connection's peer settings, if they have been retrieved.
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

    /// Run a single http/3 request/response on a bidirectional stream.
    ///
    /// Trillium runtime adapters call this once per accepted bidirectional stream, typically in a
    /// spawned task.
    pub async fn run_request<Transport, Handler, Fut>(
        self: Arc<Self>,
        transport: Transport,
        handler: Handler,
        stream_id: u64,
    ) -> Result<Conn<Transport>, H3RequestError>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnOnce(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        self.record_accepted_stream(stream_id);
        let _guard = self.swansong.guard();
        let buffer =
            Vec::with_capacity(self.server_config.http_config.request_buffer_initial_len).into();
        let conn = Conn::new_h3(self, transport, buffer).await?;
        Ok(handler(conn).await.send_h3().await?)
    }

    /// Run this server's http/3 outbound control stream.
    ///
    /// Writes the control stream type, sends SETTINGS, then sends GOAWAY when the connection
    /// swansong shuts down. Returns after GOAWAY is sent; the caller should keep the stream open
    /// until the QUIC connection closes (closing a control stream is a connection error per RFC
    /// 9114 §6.2.1).
    ///
    /// There should only be one of these per peer, but this is not currently enforced
    pub async fn outbound_control<T>(&self, mut stream: T) -> Result<(), H3RequestError>
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

    /// Run this server's outbound QPACK (headers) encoder stream.
    ///
    /// Writes the encoder stream type.
    // Currently idle (static table only); will carry encoder instructions when dynamic table is
    // added.
    pub async fn encoder<T>(&self, mut stream: T) -> Result<(), H3RequestError>
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

    /// Run this server's outbound QPACK (headers) decoder stream.
    // Writes the decoder stream type. Currently idle (static table only); will carry decoder
    // instructions when dynamic table is added.
    pub async fn decoder<T>(&self, mut stream: T) -> Result<(), H3RequestError>
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

    /// Handle an inbound unidirectional http/3 stream from the peer.
    // Reads the stream type varint and dispatches to the appropriate handler (control, QPACK
    // encoder, QPACK decoder). Unknown stream types are handled with H3_STREAM_CREATION_ERROR per
    // RFC 9114 §6.2.
    pub async fn inbound_uni<T>(
        &self,
        mut stream: T,
        stop: impl AsyncFn(T, ErrorCode),
    ) -> Result<(), H3RequestError>
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
                self.inbound_control(&mut buf, &mut filled, &mut stream)
                    .await?;
                Ok(())
            }

            Ok(UniStreamType::QpackEncoder | UniStreamType::QpackDecoder) => {
                // Static table only — hold stream open until shutdown
                self.swansong.clone().await;
                Ok(())
            }

            Ok(UniStreamType::Push) | Err(_ /* unknown stream type, discard per §6.2 */) => {
                log::trace!("{stream_type}");
                stop(stream, ErrorCode::StreamCreationError).await;
                Ok(())
            }
        }
    }

    /// Handle the http/3 peer's inbound control stream.
    // The first frame must be SETTINGS. After that, watches for
    // GOAWAY to initiate connection shutdown.
    async fn inbound_control<T>(
        &self,
        buf: &mut Vec<u8>,
        filled: &mut usize,
        stream: &mut T,
    ) -> Result<(), H3RequestError>
    where
        T: AsyncRead + Unpin + Send,
    {
        // First frame must be SETTINGS (§6.2.1)
        let settings = read(buf, filled, stream, |data| match Frame::decode(data) {
            Ok((Frame::Settings(s), consumed)) => Ok(Some((s, consumed))),
            Ok(_) => Err(ErrorCode::FrameUnexpected),
            Err(FrameDecodeError::Incomplete) => Ok(None),
            Err(FrameDecodeError::Error(code)) => Err(code),
        })
        .await?;

        self.peer_settings
            .set(settings)
            .map_err(|_| ErrorCode::FrameUnexpected)?;

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
                    return Err(ErrorCode::FrameUnexpected.into());
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
    f: impl Fn(&[u8]) -> Result<Option<(R, usize)>, ErrorCode>,
) -> Result<R, H3RequestError> {
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
