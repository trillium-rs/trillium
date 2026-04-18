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
        reason = "internal api, will revisit later"
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
            let transport = self
                .transport
                .as_deref_mut()
                .ok_or_else(|| io::Error::from(ErrorKind::NotConnected))?;
            let bytes = ready!(Pin::new(transport).poll_read(cx, buf))?;
            if bytes == 0 {
                return Ready(Err(io::Error::from(ErrorKind::UnexpectedEof)));
            }
            self.buffer.extend_from_slice(&buf[..bytes]);

            // Try to decode the frame header from the reassembled buffer
            let (remaining_in_frame, frame_type, consumed) = match Frame::decode(&self.buffer) {
                Ok((Frame::Data(len), consumed)) => (len, H3BodyFrameType::Data, consumed),
                Ok((Frame::Headers(len), consumed)) => (len, H3BodyFrameType::Trailers, consumed),
                Ok((Frame::Unknown(len), consumed)) => (len, H3BodyFrameType::Unknown, consumed),
                Err(FrameDecodeError::Incomplete) => {
                    // Still not enough bytes — stay in partial state
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
                && remaining_in_frame > self.h3_max_field_section_size
            {
                return Ready(Err(io::Error::other(H3ErrorCode::MessageError)));
            }

            let leftover = self.buffer.len();

            if leftover == 0 {
                // Frame header decoded but no payload bytes yet
                return Ready(Ok((
                    ReceivedBodyState::H3Data {
                        remaining_in_frame,
                        total,
                        frame_type,
                        partial_frame_header: false,
                    },
                    0,
                )));
            }

            // Copy leftover payload bytes from self.buffer into buf
            buf[..leftover].copy_from_slice(&self.buffer);
            self.buffer.ignore_front(leftover);

            Ready(
                H3Frame {
                    self_buffer: &mut self.buffer,
                    remaining_in_frame,
                    total,
                    frame_type,
                    buf: &mut buf[..leftover],
                    content_length: self.content_length,
                    max_len: self.max_len,
                    max_trailer_size: self.h3_max_field_section_size,
                    connection: self.h3_connection.as_ref(),
                    trailers_future: &mut self.h3_trailer_future,
                }
                .decode(),
            )
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
                    remaining_in_frame,
                    total,
                    frame_type,
                    buf: &mut buf[..bytes],
                    content_length: self.content_length,
                    max_len: self.max_len,
                    max_trailer_size: self.h3_max_field_section_size,
                    connection: self.h3_connection.as_ref(),
                    trailers_future: &mut self.h3_trailer_future,
                }
                .decode(),
            )
        }
    }
}

struct H3Frame<'a> {
    self_buffer: &'a mut Buffer,
    remaining_in_frame: u64,
    total: u64,
    frame_type: H3BodyFrameType,
    buf: &'a mut [u8],
    content_length: Option<u64>,
    max_len: u64,
    max_trailer_size: u64,
    connection: Option<&'a (Arc<H3Connection>, u64)>,
    trailers_future:
        &'a mut Option<Pin<Box<dyn Future<Output = io::Result<Headers>> + Send + Sync + 'static>>>,
}

impl H3Frame<'_> {
    #[allow(clippy::too_many_lines, reason = "internal api, will revisit later")]
    fn decode(self) -> io::Result<(ReceivedBodyState, usize)> {
        let Self {
            self_buffer,
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

        let mut ranges_to_keep = vec![];
        let mut pos = 0usize;

        let state = loop {
            // Consume bytes from the current frame
            if remaining_in_frame > 0 {
                let available = buf.len() - pos;
                let consume =
                    available.min(usize::try_from(remaining_in_frame).unwrap_or(usize::MAX));

                match frame_type {
                    H3BodyFrameType::Data => {
                        ranges_to_keep.push(pos..pos + consume);
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
                    }

                    H3BodyFrameType::Trailers => {
                        self_buffer.extend_from_slice(&buf[pos..pos + consume]);
                    }

                    _ => {}
                }

                pos += consume;
                remaining_in_frame -= consume as u64;

                // If we finished a trailers frame, body is done
                if remaining_in_frame == 0
                    && frame_type == H3BodyFrameType::Trailers
                    && let Some((connection, stream_id)) = connection
                {
                    let connection = Arc::clone(connection);
                    let stream_id = *stream_id;
                    let self_buffer = std::mem::take(self_buffer); // this is ok because there's nothing expected on a bidi stream after trailers
                    *trailers_future = Some(Box::pin(async move {
                        let field_section = connection
                            .decode_field_section(&self_buffer, stream_id)
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

            // If we've consumed the whole buffer, done for this read
            if pos >= buf.len() {
                break ReceivedBodyState::H3Data {
                    remaining_in_frame,
                    total,
                    frame_type,
                    partial_frame_header: false,
                };
            }

            // Decode next frame header
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
                    self_buffer.extend_from_slice(&buf[pos..]);
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

        // Compact: squeeze out non-body bytes
        let mut bytes = 0;
        for range in ranges_to_keep {
            let len = range.end - range.start;
            buf.copy_within(range, bytes);
            bytes += len;
        }

        Ok((state, bytes))
    }
}
