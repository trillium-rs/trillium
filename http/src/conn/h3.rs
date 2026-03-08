use crate::{
    BufWriter, Buffer, Conn, Headers, KnownHeaderName, Method, Status, TypeSet, Version,
    after_send::AfterSend,
    copy,
    h3::{ErrorCode, Frame, FrameStream, H3BodyWrapper, H3Connection, H3RequestError},
    headers::qpack::{PseudoHeaders, decode_field_section, encode_field_section},
    received_body::{H3BodyFrameType, ReceivedBodyState},
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
    pub(crate) async fn new_h3(
        h3_connection: Arc<H3Connection>,
        mut transport: Transport,
        mut buffer: Buffer,
    ) -> Result<Self, H3RequestError> {
        let start_time = Instant::now();

        let mut frame_stream = FrameStream::new(&mut transport, &mut buffer);
        let (pseudos, request_headers) = loop {
            let mut frame = frame_stream
                .next()
                .await?
                .ok_or(ErrorCode::RequestIncomplete)?;

            if let Frame::Headers(_) = frame.frame() {
                let buffered = frame.buffer_payload().await?;
                break decode_field_section(buffered).map_err(|_| ErrorCode::MessageError)?;
            }
        };

        Ok(Self::build_h3(
            h3_connection,
            transport,
            buffer,
            pseudos,
            request_headers,
            start_time,
        )?)
    }

    fn max_peer_field_section_size(&self) -> Option<u64> {
        self.h3_connection()?
            .peer_settings()?
            .max_field_section_size()
    }

    pub(crate) async fn send_h3(mut self) -> io::Result<Self> {
        self.finalize_response_headers_h3();

        let mut output_buffer =
            Vec::with_capacity(self.server_config.http_config.response_buffer_len);

        self.encode_headers_h3(&mut output_buffer)?;

        let mut bufwriter = BufWriter::new_with_buffer(output_buffer, &mut self.transport);

        if self.method != Method::Head
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(body) = self.response_body.take()
        {
            copy(
                H3BodyWrapper::new(body.0),
                &mut bufwriter,
                self.server_config.http_config.copy_loops_per_yield,
            )
            .await?;
        }

        bufwriter.flush().await?;
        self.after_send.call(true.into());
        Ok(self)
    }

    fn encode_headers_h3(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
        let pseudos = PseudoHeaders {
            status: Some(self.status.unwrap_or(Status::NotFound)),
            ..Default::default()
        };

        let mut field_section =
            Vec::with_capacity(self.server_config.http_config.request_buffer_initial_len);
        encode_field_section(&pseudos, &self.response_headers, &mut field_section);

        let size = field_section.len() as u64;
        if let Some(max_size) = self.max_peer_field_section_size()
            && size > max_size
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("headers would be longer than peer allows ({size} > {max_size})"),
            ));
        }

        let frame = Frame::Headers(field_section.len() as u64);
        let frame_header_len = frame.encoded_len();
        buffer.resize(frame_header_len, 0);
        frame.encode(buffer);
        buffer.extend_from_slice(&field_section);

        Ok(())
    }

    fn build_h3(
        h3_connection: Arc<H3Connection>,
        transport: Transport,
        buffer: Buffer,
        pseudos: PseudoHeaders<'static>,
        request_headers: Headers,
        start_time: Instant,
    ) -> Result<Self, ErrorCode> {
        let PseudoHeaders {
            method,
            path,
            authority,
            scheme,
            status: _,
            protocol,
        } = pseudos;

        if let Some(host) = request_headers.get_str(KnownHeaderName::Host)
            && let Some(authority) = &authority
            && host != authority.as_ref()
        {
            return Err(ErrorCode::MessageError);
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
            return Err(ErrorCode::MessageError);
        }

        let method = method.ok_or(ErrorCode::MessageError)?;

        match (method, &scheme) {
            (Method::Connect, None) => {}
            (Method::Connect, Some(_)) => return Err(ErrorCode::MessageError),
            (_, Some(_)) => {}
            (_, None) => return Err(ErrorCode::MessageError),
        }

        let path = match (method, path) {
            (Method::Connect, None) => Cow::Borrowed(""),
            (Method::Connect, Some(_)) => return Err(ErrorCode::MessageError),
            (_, Some(path)) => path,
            (_, None) => return Err(ErrorCode::MessageError),
        };

        if method == Method::Connect && authority.is_none() {
            return Err(ErrorCode::MessageError);
        }

        match request_headers.get_str(KnownHeaderName::Te) {
            None | Some("trailers") => {}
            _ => return Err(ErrorCode::MessageError),
        }

        let response_headers = h3_connection
            .server_config()
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        Ok(Conn {
            server_config: h3_connection.server_config(),
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
            request_body_state: ReceivedBodyState::H3Data {
                remaining_in_frame: 0,
                total: 0,
                frame_type: H3BodyFrameType::Start,
                partial_frame_header: false,
            },
            secure: true,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            authority,
            scheme,
            h3_connection: Some(h3_connection),
            protocol,
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
