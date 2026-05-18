use super::{
    AsyncRead, AsyncWrite, Context, End, ErrorKind, Ready, ReceivedBody, ReceivedBodyState,
    StateOutput, io, ready,
};
use crate::h2::H2ErrorCode;

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Read the next chunk of the body, tracking total bytes against
    /// [`max_len`][ReceivedBody::max_len] and (if declared) content-length.
    /// Transitions to [`End`] when the driver signals EOF.
    ///
    /// On content-length mismatch, signals `RST_STREAM(PROTOCOL_ERROR)` via the driver
    /// and surfaces an `io::Error` to the caller.
    #[inline]
    pub(super) fn handle_h2_data(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        total: u64,
    ) -> StateOutput {
        let bytes = ready!(self.read_raw(cx, buf)?);
        if bytes == 0 {
            return if let Some(expected) = self.content_length
                && total != expected
            {
                self.h2_protocol_error(
                    ErrorKind::InvalidData,
                    format!("content-length mismatch, {expected} != {total}"),
                )
            } else {
                Ready(Ok((End, 0)))
            };
        }

        let total = total + bytes as u64;
        if total > self.max_len {
            return self.h2_protocol_error(ErrorKind::Unsupported, "content too long".into());
        }
        if let Some(expected) = self.content_length
            && total > expected
        {
            return self.h2_protocol_error(
                ErrorKind::InvalidData,
                format!("body exceeds content-length, {total} > {expected}"),
            );
        }

        Ready(Ok((ReceivedBodyState::H2Data { total }, bytes)))
    }

    /// Signal the driver to emit `RST_STREAM(PROTOCOL_ERROR)` for this stream and return a
    /// matching `io::Error` to the caller. The caller's error and the peer-visible RST
    /// track from the same detection point.
    fn h2_protocol_error(&self, kind: ErrorKind, msg: String) -> StateOutput {
        if let Some((connection, stream_id)) = self.protocol_session.as_h2() {
            connection.stream_error(stream_id, H2ErrorCode::ProtocolError);
        }
        Ready(Err(io::Error::new(kind, msg)))
    }
}
