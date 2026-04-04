use crate::{
    BufWriter, Buffer, Conn, Headers, KnownHeaderName, Method, Status, TypeSet, Version,
    after_send::AfterSend,
    h3::{Frame, FrameStream, H3Connection, H3Error, H3ErrorCode, H3StreamResult},
    headers::qpack::{FieldSection, PseudoHeaders},
    received_body::ReceivedBodyState,
};
use futures_lite::{AsyncRead, AsyncWrite, AsyncWriteExt};
use std::{
    borrow::Cow,
    io,
    sync::Arc,
    time::{Instant, SystemTime},
};

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Parse the first frame on a bidi stream.
    ///
    /// Returns [`H3StreamResult::Request`] for normal H3 requests (HEADERS frame) or
    /// [`H3StreamResult::WebTransport`] for WebTransport bidi streams (0x41 signal).
    pub(crate) async fn new_h3(
        h3_connection: Arc<H3Connection>,
        mut transport: Transport,
        mut buffer: Buffer,
    ) -> Result<H3StreamResult<Transport>, H3Error> {
        let start_time = Instant::now();

        let mut frame_stream = FrameStream::new(&mut transport, &mut buffer);
        let field_section = loop {
            let mut frame = frame_stream
                .next()
                .await?
                .ok_or(H3ErrorCode::RequestIncomplete)?;

            match frame.frame() {
                Frame::Headers(_) => {
                    let buffered = frame.buffer_payload().await?;
                    break FieldSection::decode(buffered).map_err(|_| H3ErrorCode::MessageError)?;
                }

                Frame::WebTransport(session_id) => {
                    let session_id = *session_id;
                    drop(frame); // release borrow on frame_stream
                    return Ok(H3StreamResult::WebTransport {
                        session_id,
                        transport,
                        buffer,
                    });
                }

                _ => {}
            }
        };

        Ok(H3StreamResult::Request(Self::build_h3(
            h3_connection,
            transport,
            buffer,
            field_section,
            start_time,
        )?))
    }

    fn max_peer_field_section_size(&self) -> Option<u64> {
        self.h3_connection()?
            .peer_settings()?
            .max_field_section_size()
    }

    pub(crate) async fn send_h3(mut self) -> io::Result<Self> {
        self.finalize_response_headers_h3();

        let mut output_buffer =
            Vec::with_capacity(self.context.http_config.response_buffer_len);

        self.encode_headers_h3(&mut output_buffer)?;

        let loops_per_yield = self.context.http_config.copy_loops_per_yield;
        let max_peer_field_section_size = self.max_peer_field_section_size();
        let initial_cap = self.context.http_config.request_buffer_initial_len;

        let max_buf = self.context.http_config.response_buffer_max_len;
        let mut bufwriter = BufWriter::new_with_buffer(output_buffer, &mut self.transport, max_buf);

        if self.method != Method::Head
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(body) = self.response_body.take()
        {
            let mut body = body.into_h3();

            bufwriter.copy_from(&mut body, loops_per_yield).await?;

            if let Some(trailers) = body.trailers() {
                log::trace!("sending trailers: {trailers}");
                encode_field_section_h3(
                    &FieldSection::new(PseudoHeaders::default(), &trailers),
                    max_peer_field_section_size,
                    initial_cap,
                    bufwriter.buffer_mut(),
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
        encode_field_section_h3(
            &field_section,
            self.max_peer_field_section_size(),
            self.context.http_config.request_buffer_initial_len,
            buffer,
        )
    }

    fn build_h3(
        h3_connection: Arc<H3Connection>,
        transport: Transport,
        buffer: Buffer,
        mut field_section: FieldSection<'static>,
        start_time: Instant,
    ) -> Result<Self, H3ErrorCode> {
        log::trace!("received:\n{field_section}");
        let pseudo_headers = field_section.pseudo_headers_mut();

        let method = pseudo_headers.take_method();
        let path = pseudo_headers.take_path();
        let authority = pseudo_headers.take_authority();
        let scheme = pseudo_headers.take_scheme();
        let protocol = pseudo_headers.take_protocol();

        let request_headers = field_section.into_headers().into_owned();

        if let Some(host) = request_headers.get_str(KnownHeaderName::Host)
            && let Some(authority) = &authority
            && host != authority.as_ref()
        {
            return Err(H3ErrorCode::MessageError);
        }

        if [
            KnownHeaderName::Connection,
            KnownHeaderName::KeepAlive,
            KnownHeaderName::ProxyConnection,
            KnownHeaderName::TransferEncoding,
            KnownHeaderName::Upgrade,
        ]
        .into_iter()
        .any(|name| request_headers.has_header(name))
        {
            return Err(H3ErrorCode::MessageError);
        }

        let method = method.ok_or(H3ErrorCode::MessageError)?;

        if method != Method::Connect && scheme.is_none() {
            return Err(H3ErrorCode::MessageError);
        }

        let path = match (method, path) {
            (_, Some(path)) => path,
            (Method::Connect, None) => Cow::Borrowed("/"),
            (_, None) => return Err(H3ErrorCode::MessageError),
        };

        if method == Method::Connect && authority.is_none() {
            return Err(H3ErrorCode::MessageError);
        }

        match request_headers.get_str(KnownHeaderName::Te) {
            None | Some("trailers") => {}
            _ => return Err(H3ErrorCode::MessageError),
        }

        let response_headers = h3_connection
            .context()
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        Ok(Conn {
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
            request_body_state: ReceivedBodyState::new_h3(),
            secure: true,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            authority,
            scheme,
            h3_connection: Some(h3_connection),
            protocol,
            request_trailers: None,
        })
    }

    pub(super) fn finalize_response_headers_h3(&mut self) {
        self.response_headers
            .try_insert_with(KnownHeaderName::Date, || {
                httpdate::fmt_http_date(SystemTime::now())
            });

        if !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(len) = self.body_len()
        {
            self.response_headers
                .try_insert(KnownHeaderName::ContentLength, len);
        }

        self.response_headers.remove_all([
            KnownHeaderName::Connection,
            KnownHeaderName::TransferEncoding,
            KnownHeaderName::KeepAlive,
            KnownHeaderName::ProxyConnection,
            KnownHeaderName::Upgrade,
        ]);
    }
}

fn encode_field_section_h3(
    field_section: &FieldSection<'_>,
    max_peer_field_section_size: Option<u64>,
    initial_cap: usize,
    buffer: &mut Vec<u8>,
) -> io::Result<()> {
    let mut field_section_buf = Vec::with_capacity(initial_cap);
    field_section.encode(&mut field_section_buf);

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
    buffer.resize(frame_header_len, 0);
    frame.encode(buffer);
    buffer.extend_from_slice(&field_section_buf);

    Ok(())
}
