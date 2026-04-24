use super::{
    AsyncRead, AsyncWrite, Context, End, ErrorKind, Ready, ReceivedBody, ReceivedBodyState,
    StateOutput, io, ready,
};
use crate::h2::H2ErrorCode;

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Read the next chunk of the body from the `H2Transport`, tracking total bytes against
    /// [`max_len`][ReceivedBody::max_len] and (if declared) content-length. Transitions to
    /// [`End`] when the driver signals EOF — `Ready(0)` out of the recv ring after the peer
    /// set `END_STREAM` on a DATA (or trailing HEADERS) frame.
    ///
    /// Structurally simpler than [`handle_h3_data`][ReceivedBody::handle_h3_data]: the h2
    /// driver demuxes DATA frames into a per-stream recv ring on a separate task, so by the
    /// time `read_raw` yields bytes they are guaranteed to be body payload with no frame
    /// boundaries to parse. h3 by contrast hands us raw QUIC-stream bytes that still carry h3
    /// frame headers. Trailers on h2 are delivered through [`H2Connection::take_trailers`] in
    /// the `End` transition on [`ReceivedBody::poll_read`]; nothing for this arm to do.
    ///
    /// When this path detects a protocol violation (content-length mismatch per RFC 9113
    /// §8.1.2.6), it signals the driver via
    /// [`H2Connection::stream_error`][crate::h2::H2Connection::stream_error] so an
    /// `RST_STREAM(PROTOCOL_ERROR)` lands on the wire, then surfaces the same failure as an
    /// `io::Error` to the caller. The caller's `io::Error` and the peer-visible RST both
    /// track from the same detection point — no need for the caller to know about the wire
    /// side.
    ///
    /// [`H2Connection::take_trailers`]: crate::h2::H2Connection::take_trailers
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
                self.signal_h2_stream_error(H2ErrorCode::ProtocolError);
                Ready(Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("content-length mismatch, {expected} != {total}"),
                )))
            } else {
                Ready(Ok((End, 0)))
            };
        }

        let total = total + bytes as u64;
        if total > self.max_len {
            self.signal_h2_stream_error(H2ErrorCode::ProtocolError);
            return Ready(Err(io::Error::new(
                ErrorKind::Unsupported,
                "content too long",
            )));
        }
        if let Some(expected) = self.content_length
            && total > expected
        {
            self.signal_h2_stream_error(H2ErrorCode::ProtocolError);
            return Ready(Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("body exceeds content-length, {total} > {expected}"),
            )));
        }

        Ready(Ok((ReceivedBodyState::H2Data { total }, bytes)))
    }

    /// Request the h2 driver `RST_STREAM` this body's stream. No-op if we were constructed
    /// without an `h2_connection` (shouldn't happen for the h2-data arm, but keeps the
    /// helper defensive).
    fn signal_h2_stream_error(&self, code: H2ErrorCode) {
        if let Some((connection, stream_id)) = &self.h2_connection {
            connection.stream_error(*stream_id, code);
        }
    }
}
