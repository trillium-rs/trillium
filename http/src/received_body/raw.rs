use super::{
    AsyncRead, AsyncWrite, Context, End, ErrorKind, Ready, ReceivedBody, ReceivedBodyState,
    StateOutput, io, ready,
};
use crate::h2::H2ErrorCode;

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Read the next chunk of body bytes from a transport that already yields plain
    /// payload (h2 driver-demuxed bodies, h1.0 read-to-close responses, raw upgrade
    /// transports). Tracks total bytes against [`max_len`][ReceivedBody::max_len] and
    /// (if declared) content-length. Transitions to [`End`] on EOF.
    ///
    /// A content-length mismatch surfaces `io::Error` to the caller, and on h2 sessions
    /// also signals `RST_STREAM(PROTOCOL_ERROR)` via the driver.
    #[inline]
    pub(super) fn handle_raw(
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
        if let Some(expected) = self.content_length
            && total > expected
        {
            return self.protocol_error(
                ErrorKind::InvalidData,
                format!("body exceeds content-length, {total} > {expected}"),
            );
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
