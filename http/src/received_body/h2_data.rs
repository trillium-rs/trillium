use super::{
    AsyncRead, AsyncWrite, Context, End, ErrorKind, Ready, ReceivedBody, ReceivedBodyState,
    StateOutput, io, ready,
};

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
            return Ready(Err(io::Error::new(
                ErrorKind::Unsupported,
                "content too long",
            )));
        }
        if let Some(expected) = self.content_length
            && total > expected
        {
            return Ready(Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("body exceeds content-length, {total} > {expected}"),
            )));
        }

        Ready(Ok((ReceivedBodyState::H2Data { total }, bytes)))
    }
}
