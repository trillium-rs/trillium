use super::{
    AsyncRead, AsyncWrite, Context, End, ErrorKind, Ready, ReceivedBody, ReceivedBodyState,
    StateOutput, io, ready,
};
use crate::{ProtocolSession, h2::H2ErrorCode};

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Per-state handler for [`ReceivedBodyState::Raw`]. When `content_length` is set,
    /// the read buffer is clamped to the remaining declared bytes so any pipelined
    /// bytes past the body boundary stay in the transport's buffered prefix, and the
    /// state transitions to [`End`] once the declared length is reached. Without a
    /// declared length, transitions to [`End`] on EOF. Content-length mismatches
    /// surface as `InvalidData` and, on h2, also signal `RST_STREAM(PROTOCOL_ERROR)`.
    #[inline]
    pub(super) fn handle_raw(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        total: u64,
    ) -> StateOutput {
        // Whether `content-length` *frames* the body (so it's terminal) or is merely advisory.
        // In h1, content-length defines the body length and the next request's bytes follow on
        // the same transport, so we clamp reads to the declared length and end on it. In h2/h3
        // the driver demuxes DATA per stream and the authoritative end-of-body is the stream's
        // `END_STREAM` — after which a trailing HEADERS frame may still deliver trailers. Ending
        // on content-length there would complete the body *before* the trailers arrive and drop
        // them (the gRPC unary/client-stream "empty trailers → Unknown" race). content-length
        // stays a cross-check, validated against the byte total at EOF below.
        let length_is_terminal = matches!(self.protocol_session, ProtocolSession::Http1);

        let buf = if let Some(expected) = self.content_length.filter(|_| length_is_terminal) {
            let remaining = usize::try_from(expected - total).unwrap_or(usize::MAX);
            let len = buf.len().min(remaining);
            &mut buf[..len]
        } else {
            buf
        };

        let bytes = ready!(self.read_raw(cx, buf)?);
        if bytes == 0 {
            return if let Some(expected) = self.content_length
                && total != expected
            {
                self.protocol_error(
                    ErrorKind::InvalidData,
                    format!("content-length mismatch, {expected} != {total}"),
                )
            } else {
                Ready(Ok((End, 0)))
            };
        }

        let total = total + bytes as u64;
        if total > self.max_len {
            return self.protocol_error(ErrorKind::Unsupported, "content too long".into());
        }
        if length_is_terminal
            && let Some(expected) = self.content_length
            && total == expected
        {
            return Ready(Ok((End, bytes)));
        }

        Ready(Ok((ReceivedBodyState::Raw { total }, bytes)))
    }

    /// Surface an `io::Error` to the caller, and on h2 sessions also signal
    /// `RST_STREAM(PROTOCOL_ERROR)` so caller-error and peer-visible RST share one
    /// detection point. No-op for h1.0 read-to-close and raw upgrades, which have no
    /// per-stream control channel.
    fn protocol_error(&self, kind: ErrorKind, msg: String) -> StateOutput {
        if let Some((connection, stream_id)) = self.protocol_session.as_h2() {
            connection.stream_error(stream_id, H2ErrorCode::ProtocolError);
        }
        Ready(Err(io::Error::new(kind, msg)))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Buffer, HttpConfig, ReceivedBody, ReceivedBodyState};
    use encoding_rs::UTF_8;
    use futures_lite::{AsyncRead, AsyncReadExt, future::block_on, io::Cursor};

    fn new_with_config(
        input: String,
        config: &HttpConfig,
    ) -> ReceivedBody<'static, Cursor<Vec<u8>>> {
        ReceivedBody::new_with_config(
            Some(input.len() as u64),
            Buffer::with_capacity(100),
            Cursor::new(input.into_bytes()),
            ReceivedBodyState::Raw { total: 0 },
            None,
            UTF_8,
            config,
        )
    }

    fn decode_with_config(
        input: String,
        poll_size: usize,
        config: &HttpConfig,
    ) -> crate::Result<String> {
        let mut rb = new_with_config(input, config);

        block_on(read_with_buffers_of_size(&mut rb, poll_size))
    }

    async fn read_with_buffers_of_size<R>(reader: &mut R, size: usize) -> crate::Result<String>
    where
        R: AsyncRead + Unpin,
    {
        let mut return_buffer = vec![];
        loop {
            let mut buf = vec![0; size];
            match reader.read(&mut buf).await? {
                0 => break Ok(String::from_utf8_lossy(&return_buffer).into()),
                bytes_read => return_buffer.extend_from_slice(&buf[..bytes_read]),
            }
        }
    }

    #[test]
    fn test() {
        for size in 3..50 {
            let input = "12345abcdef";
            let output = decode_with_config(input.into(), size, &HttpConfig::DEFAULT).unwrap();
            assert_eq!(output, "12345abcdef", "size: {size}");

            let input = "MozillaDeveloperNetwork";
            let output = decode_with_config(input.into(), size, &HttpConfig::DEFAULT).unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork", "size: {size}");

            assert!(decode_with_config(String::new(), size, &HttpConfig::DEFAULT).is_ok());

            let input = "MozillaDeveloperNetwork";
            assert!(
                decode_with_config(
                    input.into(),
                    size,
                    &HttpConfig::DEFAULT.with_received_body_max_len(5)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn read_string_and_read_bytes() {
        block_on(async {
            let content = "test ".repeat(1000);
            assert_eq!(
                new_with_config(content.clone(), &HttpConfig::DEFAULT)
                    .read_string()
                    .await
                    .unwrap()
                    .len(),
                5000
            );

            assert_eq!(
                new_with_config(content.clone(), &HttpConfig::DEFAULT)
                    .read_bytes()
                    .await
                    .unwrap()
                    .len(),
                5000
            );

            assert!(
                new_with_config(
                    content.clone(),
                    &HttpConfig::DEFAULT.with_received_body_max_len(750)
                )
                .read_string()
                .await
                .is_err()
            );

            assert!(
                new_with_config(
                    content.clone(),
                    &HttpConfig::DEFAULT.with_received_body_max_len(750)
                )
                .read_bytes()
                .await
                .is_err()
            );

            assert!(
                new_with_config(content.clone(), &HttpConfig::DEFAULT)
                    .with_max_len(750)
                    .read_bytes()
                    .await
                    .is_err()
            );

            assert!(
                new_with_config(content.clone(), &HttpConfig::DEFAULT)
                    .with_max_len(750)
                    .read_string()
                    .await
                    .is_err()
            );
        });
    }
}
