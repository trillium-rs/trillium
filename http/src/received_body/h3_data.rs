use super::{
    AsyncRead, AsyncWrite, Buffer, Context, End, ErrorKind, H3BodyFrameType, Pin, Ready,
    ReceivedBody, ReceivedBodyState, StateOutput, io, ready,
};
use crate::{
    Headers,
    h3::{Frame, FrameDecodeError, H3Connection, H3ErrorCode},
};
use std::sync::Arc;

#[cfg(test)]
mod tests;

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    #[inline]
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "single state-machine arm; splitting would obscure the control flow"
    )]
    pub(super) fn handle_h3_data(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        remaining_in_frame: u64,
        total: u64,
        frame_type: H3BodyFrameType,
        partial_frame_header: bool,
    ) -> StateOutput {
        if partial_frame_header {
            // Decode the next frame header. It may already be fully buffered — e.g. it
            // arrived in the same read as the response headers and was over-read into
            // `buffer`. Only pull from the transport when the buffer can't yet complete the
            // header; otherwise an idle stream (peer waiting on us) would hang a read, or a
            // closed one would raise a spurious EOF, that the buffer could already satisfy.
            if matches!(
                Frame::decode(&self.buffer),
                Err(FrameDecodeError::Incomplete)
            ) {
                let transport = self
                    .transport
                    .as_deref_mut()
                    .ok_or_else(|| io::Error::from(ErrorKind::NotConnected))?;
                let bytes = ready!(Pin::new(transport).poll_read(cx, buf))?;
                if bytes == 0 {
                    return Ready(Err(io::Error::from(ErrorKind::UnexpectedEof)));
                }
                self.buffer.extend_from_slice(&buf[..bytes]);
            }

            let (remaining_in_frame, frame_type, consumed) = match Frame::decode(&self.buffer) {
                Ok((Frame::Data(len), consumed)) => (len, H3BodyFrameType::Data, consumed),
                Ok((Frame::Headers(len), consumed)) => (len, H3BodyFrameType::Trailers, consumed),
                Ok((Frame::Unknown(len), consumed)) => (len, H3BodyFrameType::Unknown, consumed),
                Err(FrameDecodeError::Incomplete) => {
                    // Read some bytes but still short of a full header — stay partial.
                    return Ready(Ok((
                        ReceivedBodyState::H3Data {
                            remaining_in_frame: 0,
                            total,
                            frame_type: H3BodyFrameType::Start,
                            partial_frame_header: true,
                        },
                        0,
                    )));
                }
                Err(FrameDecodeError::Error(_)) => {
                    return Ready(Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "H3 frame error",
                    )));
                }
                Ok(_err) => {
                    return Ready(Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "Unexpected frame type on request stream",
                    )));
                }
            };

            self.buffer.ignore_front(consumed);

            if frame_type == H3BodyFrameType::Trailers
                && remaining_in_frame > self.max_header_list_size
            {
                return Ready(Err(io::Error::other(H3ErrorCode::MessageError)));
            }

            // Header complete. Hand the payload off to the non-partial path on the next poll
            // iteration, which drains it from the buffer (then transport) honoring `buf`'s
            // length — rather than assuming `buf` can hold the whole buffered frame.
            Ready(Ok((
                ReceivedBodyState::H3Data {
                    remaining_in_frame,
                    total,
                    frame_type,
                    partial_frame_header: false,
                },
                0,
            )))
        } else {
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

            Ready(
                H3Frame {
                    self_buffer: &mut self.buffer,
                    trailer_payload_buffer: &mut self.h3_trailer_payload_buffer,
                    remaining_in_frame,
                    total,
                    frame_type,
                    buf: &mut buf[..bytes],
                    content_length: self.content_length,
                    max_len: self.max_len,
                    max_trailer_size: self.max_header_list_size,
                    connection: self.protocol_session.as_h3_borrowed(),
                    trailers_future: &mut self.h3_trailer_future,
                }
                .decode(),
            )
        }
    }
}

struct H3Frame<'a> {
    /// Pre-read transport bytes and frame-header reassembly scratch for headers split
    /// across reads.
    self_buffer: &'a mut Buffer,
    /// Accumulator for the QPACK-encoded trailer payload. Separate from `self_buffer` so
    /// the read-buffered drain on subsequent polls doesn't recycle accumulated trailer
    /// bytes back through the frame decoder.
    trailer_payload_buffer: &'a mut Vec<u8>,
    remaining_in_frame: u64,
    total: u64,
    frame_type: H3BodyFrameType,
    buf: &'a mut [u8],
    content_length: Option<u64>,
    max_len: u64,
    max_trailer_size: u64,
    connection: Option<(&'a Arc<H3Connection>, u64)>,
    trailers_future:
        &'a mut Option<Pin<Box<dyn Future<Output = io::Result<Headers>> + Send + Sync + 'static>>>,
}

impl H3Frame<'_> {
    #[allow(
        clippy::too_many_lines,
        reason = "single state-machine arm; splitting would obscure the control flow"
    )]
    fn decode(self) -> io::Result<(ReceivedBodyState, usize)> {
        let Self {
            self_buffer,
            trailer_payload_buffer,
            mut remaining_in_frame,
            mut total,
            mut frame_type,
            buf,
            content_length,
            max_len,
            max_trailer_size,
            connection,
            trailers_future,
            ..
        } = self;
        if buf.is_empty() {
            return Err(io::Error::from(ErrorKind::UnexpectedEof));
        }

        let mut bytes = 0usize;
        let mut pos = 0usize;

        let state = loop {
            if remaining_in_frame > 0 {
                let available = buf.len() - pos;
                let consume =
                    available.min(usize::try_from(remaining_in_frame).unwrap_or(usize::MAX));

                match frame_type {
                    H3BodyFrameType::Data => {
                        total += consume as u64;

                        if total > max_len {
                            return Err(io::Error::new(ErrorKind::Unsupported, "content too long"));
                        }

                        if let Some(expected) = content_length
                            && total > expected
                        {
                            return Err(io::Error::new(
                                ErrorKind::InvalidData,
                                format!("body exceeds content-length, {total} > {expected}"),
                            ));
                        }

                        // Compact in place as each payload segment completes: the write
                        // cursor (`bytes`) can't exceed `pos` because kept bytes are a
                        // subset of already-parsed bytes, and everything not yet parsed
                        // lies at or beyond `pos + consume` — so this slide never
                        // clobbers unparsed input.
                        if pos != bytes {
                            buf.copy_within(pos..pos + consume, bytes);
                        }
                        bytes += consume;
                    }

                    H3BodyFrameType::Trailers => {
                        trailer_payload_buffer.extend_from_slice(&buf[pos..pos + consume]);
                    }

                    _ => {}
                }

                pos += consume;
                remaining_in_frame -= consume as u64;

                if remaining_in_frame == 0
                    && frame_type == H3BodyFrameType::Trailers
                    && let Some((connection, stream_id)) = connection
                {
                    let connection = Arc::clone(connection);
                    let encoded = std::mem::take(trailer_payload_buffer);
                    *trailers_future = Some(Box::pin(async move {
                        let field_section = connection
                            .decode_field_section(&encoded, stream_id)
                            .await
                            .map_err(|e| {
                                log::error!(
                                    "h3 error encountered in trailers that we are unable to \
                                     respond to: {e:?}"
                                );
                                io::Error::new(
                                    ErrorKind::InvalidData,
                                    "invalid trailer field section",
                                )
                            })?;
                        Ok(field_section.into_headers().into_owned())
                    }));

                    if let Some(expected) = content_length
                        && total != expected
                    {
                        return Err(io::Error::new(
                            ErrorKind::InvalidData,
                            "content-length mismatch",
                        ));
                    }
                    break End;
                }
            }

            if pos >= buf.len() {
                break ReceivedBodyState::H3Data {
                    remaining_in_frame,
                    total,
                    frame_type,
                    partial_frame_header: false,
                };
            }

            match Frame::decode(&buf[pos..]) {
                Ok((Frame::Data(len), consumed)) => {
                    pos += consumed;
                    remaining_in_frame = len;
                    frame_type = H3BodyFrameType::Data;
                }

                Ok((Frame::Headers(len), consumed)) => {
                    if len > max_trailer_size {
                        return Err(io::Error::other(H3ErrorCode::MessageError));
                    }
                    pos += consumed;
                    remaining_in_frame = len;
                    frame_type = H3BodyFrameType::Trailers;
                }

                Ok((Frame::Unknown(len), consumed)) => {
                    pos += consumed;
                    remaining_in_frame = len;
                    frame_type = H3BodyFrameType::Unknown;
                }

                Err(FrameDecodeError::Incomplete) => {
                    // These bytes were drained from the *front* of `self_buffer` (or read
                    // from the transport) and precede whatever remains buffered, so they go
                    // back at the front — appending would reorder them after the remainder.
                    self_buffer.prepend(&buf[pos..]);
                    break ReceivedBodyState::H3Data {
                        remaining_in_frame: 0,
                        total,
                        frame_type: H3BodyFrameType::Start,
                        partial_frame_header: true,
                    };
                }

                Err(FrameDecodeError::Error(_code)) => {
                    return Err(io::Error::new(ErrorKind::InvalidData, "H3 frame error"));
                }

                Ok(_) => {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "unexpected frame type on request stream",
                    ));
                }
            }
        };

        Ok((state, bytes))
    }
}
