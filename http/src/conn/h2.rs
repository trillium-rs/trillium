use crate::{
    Buffer, Conn, Headers, KnownHeaderName, Method, TypeSet, Version,
    after_send::AfterSend,
    h2::{H2Connection, H2ErrorCode},
    headers::hpack::FieldSection,
    received_body::ReceivedBodyState,
};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{borrow::Cow, sync::Arc, time::Instant};

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Build a `Conn` from the bits an HTTP/2 stream-open hands the driver: the shared
    /// connection, the stream id, the decoded request field section, and a transport (today an
    /// `H2Transport`; will become a ZST placeholder in a later step).
    ///
    /// Synchronous — no I/O required at this point because the driver has already decoded the
    /// HEADERS block. Returns an [`H2ErrorCode`] for any RFC 9113 §8.1.2 malformed-request
    /// condition; the driver maps these to a stream-level `RST_STREAM` and never spawns a
    /// handler task for the rejected stream.
    ///
    /// # Errors
    ///
    /// Returns [`H2ErrorCode::ProtocolError`] for any §8.1.2 violation: missing required
    /// pseudo-header, empty `:path` outside CONNECT, missing `:authority` for CONNECT, presence
    /// of any HTTP/1-only connection-specific header, `Host` mismatch with `:authority`, or a
    /// `TE` header with a value other than `trailers`.
    pub(crate) fn new_h2(
        h2_connection: Arc<H2Connection>,
        stream_id: u32,
        mut request_headers: FieldSection<'static>,
        transport: Transport,
    ) -> Result<Self, H2ErrorCode> {
        log::trace!("h2 stream {stream_id}: building Conn from\n{request_headers}");
        let pseudo_headers = request_headers.pseudo_headers_mut();

        let method = pseudo_headers.take_method();
        let path = pseudo_headers.take_path();
        let authority = pseudo_headers.take_authority();
        let scheme = pseudo_headers.take_scheme();
        let protocol = pseudo_headers.take_protocol();

        let request_headers = request_headers.into_headers().into_owned();

        if let Some(host) = request_headers.get_str(KnownHeaderName::Host)
            && let Some(authority) = &authority
            && host != authority.as_ref()
        {
            return Err(H2ErrorCode::ProtocolError);
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
            return Err(H2ErrorCode::ProtocolError);
        }

        let method = method.ok_or(H2ErrorCode::ProtocolError)?;

        if method != Method::Connect && scheme.is_none() {
            return Err(H2ErrorCode::ProtocolError);
        }

        let path = match (method, path) {
            (_, Some(path)) if !path.is_empty() => path,
            (Method::Connect, _) => Cow::Borrowed("/"),
            _ => return Err(H2ErrorCode::ProtocolError),
        };

        if method == Method::Connect && authority.is_none() {
            return Err(H2ErrorCode::ProtocolError);
        }

        match request_headers.get_str(KnownHeaderName::Te) {
            None | Some("trailers") => {}
            _ => return Err(H2ErrorCode::ProtocolError),
        }

        let response_headers = h2_connection
            .context()
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        Ok(Conn {
            context: h2_connection.context(),
            transport,
            request_headers,
            method,
            version: Version::Http2,
            path,
            buffer: Buffer::default(),
            response_headers,
            status: None,
            state: TypeSet::new(),
            response_body: None,
            request_body_state: ReceivedBodyState::Start,
            secure: true,
            after_send: AfterSend::default(),
            start_time: Instant::now(),
            peer_ip: None,
            authority,
            scheme,
            h3_connection: None,
            h3_stream_id: None,
            h2_connection: Some(h2_connection),
            h2_stream_id: Some(stream_id),
            protocol,
            request_trailers: None,
        })
    }
}
