use crate::{
    BufWriter, Buffer, Conn, Headers, KnownHeaderName, Method, ProtocolSession, Status, TypeSet,
    Version,
    after_send::AfterSend,
    h3::{Frame, FrameStream, H3Connection, H3Error, H3ErrorCode},
    headers::qpack::{FieldSection, PseudoHeaders},
    received_body::ReceivedBodyState,
};
use futures_lite::{AsyncRead, AsyncWrite, AsyncWriteExt};
use std::{
    io,
    sync::Arc,
    time::{Instant, SystemTime},
};

/// Resolution of an H3 bidi stream's first frame. Returned before `Conn`
/// construction so the caller can issue a stream RST on the error path.
pub(crate) enum H3FirstFrame {
    /// First frame was HEADERS, decoded and validated.
    Request {
        validated: super::ValidatedRequest,
        start_time: Instant,
    },
    /// First "frame" was the WebTransport 0x41 bidi-stream signal.
    WebTransport { session_id: u64 },
}

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Read and classify the first frame on an H3 bidi stream.
    ///
    /// Any first frame other than HEADERS or the WebTransport signal — including a
    /// HEADERS frame that fails to decode — is `H3_FRAME_UNEXPECTED`. I/O errors pass
    /// through.
    pub(crate) async fn process_first_frame_h3(
        h3_connection: &H3Connection,
        transport: &mut Transport,
        buffer: &mut Buffer,
        stream_id: u64,
    ) -> Result<H3FirstFrame, H3Error> {
        log::trace!("H3 bidi stream {stream_id}: started");
        let start_time = Instant::now();
        log::trace!("H3 bidi stream {stream_id}: waiting for first frame");

        let field_section = {
            let mut frame_stream = FrameStream::new(transport, buffer);
            let mut frame = frame_stream
                .next()
                .await
                .map_err(|e| match e {
                    H3Error::Protocol(_) => H3ErrorCode::FrameUnexpected.into(),
                    io @ H3Error::Io(_) => io,
                })?
                .ok_or(H3ErrorCode::RequestIncomplete)?;

            match frame.frame() {
                Frame::Headers(_) => {
                    log::trace!("H3 bidi stream {stream_id}: decoding HEADERS frame");
                    let encoded = frame.buffer_payload().await?;
                    let result = h3_connection
                        .decode_field_section(encoded, stream_id)
                        .await
                        .inspect_err(|e| {
                            log::debug!("H3 bidi stream {stream_id}: HEADERS decode error: {e:?}");
                        })?;
                    log::trace!("H3 bidi stream {stream_id}: HEADERS decoded:\n{result}");
                    result
                }

                Frame::WebTransport(session_id) => {
                    let session_id = *session_id;
                    return Ok(H3FirstFrame::WebTransport { session_id });
                }

                other => {
                    log::trace!("H3 bidi stream {stream_id}: unexpected first frame {other:?}");
                    return Err(H3ErrorCode::FrameUnexpected.into());
                }
            }
        };

        log::trace!("received:\n{field_section}");
        let validated =
            super::validate_h2h3_request(field_section).ok_or(H3ErrorCode::MessageError)?;
        Ok(H3FirstFrame::Request {
            validated,
            start_time,
        })
    }

    fn max_peer_field_section_size(&self) -> Option<u64> {
        self.h3_connection()?
            .peer_settings()?
            .max_field_section_size()
    }

    pub(crate) async fn send_h3(mut self) -> io::Result<Self> {
        self.finalize_response_headers_h3();
        let mut output_buffer = Vec::with_capacity(self.context.config.response_buffer_len);

        self.encode_headers_h3(&mut output_buffer)?;

        // Read before the bufwriter borrows `self.transport`. On an upgrade the response
        // body is a prelude on the still-open stream, so its trailers belong at the eventual
        // close (carried via `Upgrade`), not inline.
        let upgrading = self.should_upgrade();

        let loops_per_yield = self.context.config.copy_loops_per_yield;
        let max_peer_field_section_size = self.max_peer_field_section_size();
        let initial_cap = self.context.config.request_buffer_initial_len;

        let max_buf = self.context.config.response_buffer_max_len;
        let mut bufwriter = BufWriter::new_with_buffer(output_buffer, &mut self.transport, max_buf);

        if self.method != Method::Head
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(body) = self.response_body.take()
        {
            let mut body = body.into_h3();

            bufwriter.copy_from(&mut body, loops_per_yield).await?;

            if !upgrading && let Some(trailers) = body.trailers() {
                let Some((h3, stream_id)) = self.protocol_session.as_h3() else {
                    return Err(io::ErrorKind::NotConnected.into());
                };

                log::trace!("sending trailers: {trailers}");
                encode_field_section_h3(
                    &h3,
                    &FieldSection::new(PseudoHeaders::default(), &trailers),
                    max_peer_field_section_size,
                    initial_cap,
                    bufwriter.buffer_mut(),
                    stream_id,
                )?;
            }
        }

        bufwriter.flush().await?;
        self.after_send.call(true.into());
        Ok(self)
    }

    fn encode_headers_h3(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
        let pseudo_headers =
            PseudoHeaders::default().with_status(self.status.unwrap_or(Status::NotFound));

        let field_section = FieldSection::new(pseudo_headers, &self.response_headers);
        log::trace!("sending:\n{field_section}");
        let Some((h3, stream_id)) = self.protocol_session.as_h3() else {
            return Err(io::ErrorKind::NotConnected.into());
        };

        encode_field_section_h3(
            &h3,
            &field_section,
            self.max_peer_field_section_size(),
            self.context.config.request_buffer_initial_len,
            buffer,
            stream_id,
        )
    }

    pub(crate) fn build_h3(
        h3_connection: Arc<H3Connection>,
        transport: Transport,
        buffer: Buffer,
        validated: super::ValidatedRequest,
        start_time: Instant,
        stream_id: u64,
    ) -> Self {
        let super::ValidatedRequest {
            method,
            path,
            authority,
            scheme,
            protocol,
            request_headers,
        } = validated;

        let response_headers = h3_connection
            .context()
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        let request_body_state = ReceivedBodyState::new_h3();

        Conn {
            context: h3_connection.context(),
            transport,
            request_headers,
            method,
            version: Version::Http3,
            path,
            buffer,
            response_headers,
            status: None,
            state: TypeSet::new(),
            response_body: None,
            request_body_state,
            secure: true,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            authority,
            scheme,
            protocol,
            protocol_session: ProtocolSession::Http3 {
                connection: h3_connection,
                stream_id,
            },
            request_trailers: None,
            upgrade: false,
        }
    }

    /// Apply h3-flavored finalizations to the response headers: insert a Date header if absent,
    /// surface content-length if known, strip h1-only connection-management headers (forbidden
    /// in h3 per RFC 9114).
    ///
    /// Skips Content-Length insertion on the extended-CONNECT upgrade path: the response is
    /// HEADERS-only with the QUIC stream staying open as a bidi byte channel.
    ///
    /// Parallel to `finalize_response_headers_1x` (h1) and `finalize_response_headers_h2`
    /// (h2); keep the three in sync when changing universal policy.
    pub(super) fn finalize_response_headers_h3(&mut self) {
        self.response_headers
            .try_insert_with(KnownHeaderName::Date, || {
                httpdate::fmt_http_date(SystemTime::now())
            });

        if !self.should_upgrade()
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(len) = self.body_len()
        {
            self.response_headers
                .try_insert(KnownHeaderName::ContentLength, len);
        }

        self.response_headers.remove_all(super::H1_ONLY_HEADERS);
    }
}

/// Encode an HTTP/3 HEADERS frame (type+length prefix followed by the QPACK-encoded
/// field section) and append it to `buffer`.
///
/// # Errors
///
/// Returns the QPACK encoder's error if encoding fails, or [`io::ErrorKind::InvalidData`]
/// if the field-section size exceeds the peer's `SETTINGS_MAX_FIELD_SECTION_SIZE`.
pub(crate) fn encode_field_section_h3(
    h3: &H3Connection,
    field_section: &FieldSection<'_>,
    max_peer_field_section_size: Option<u64>,
    initial_cap: usize,
    buffer: &mut Vec<u8>,
    stream_id: u64,
) -> io::Result<()> {
    let mut field_section_buf = Vec::with_capacity(initial_cap);
    h3.encode_field_section(field_section, &mut field_section_buf, stream_id)
        .map_err(|error| {
            log::error!("encode error: {error:?}");
            io::Error::other(error)
        })?;

    let size = field_section_buf.len() as u64;
    if let Some(max_size) = max_peer_field_section_size
        && size > max_size
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("field section would be longer than peer allows ({size} > {max_size})"),
        ));
    }

    let frame = Frame::Headers(field_section_buf.len() as u64);
    let frame_header_len = frame.encoded_len();
    let start = buffer.len();
    buffer.resize(start + frame_header_len, 0);
    frame.encode(&mut buffer[start..]);
    buffer.extend_from_slice(&field_section_buf);

    Ok(())
}
