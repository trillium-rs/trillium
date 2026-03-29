use super::{
    AsyncRead, Buffer, Context, End, ErrorKind, H3BodyFrameType, Pin, Ready, ReceivedBody,
    ReceivedBodyState, StateOutput, io, ready,
};
use crate::{
    Headers,
    h3::{Frame, FrameDecodeError, H3ErrorCode},
    headers::qpack::FieldSection,
};

#[cfg(test)]
mod tests;

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    #[inline]
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

            Ready(h3_frame_decode(
                &mut self.buffer,
                remaining_in_frame,
                total,
                frame_type,
                &mut buf[..leftover],
                self.content_length,
                self.max_len,
                self.h3_max_field_section_size,
                &mut self.trailers,
            ))
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

            Ready(h3_frame_decode(
                &mut self.buffer,
                remaining_in_frame,
                total,
                frame_type,
                &mut buf[..bytes],
                self.content_length,
                self.max_len,
                self.h3_max_field_section_size,
                &mut self.trailers,
            ))
        }
    }
}

fn h3_frame_decode(
    self_buffer: &mut Buffer,
    mut remaining_in_frame: u64,
    mut total: u64,
    mut frame_type: H3BodyFrameType,
    buf: &mut [u8],
    content_length: Option<u64>,
    max_len: u64,
    max_trailer_size: u64,
    trailers: &mut Option<Headers>,
) -> io::Result<(ReceivedBodyState, usize)> {
    if buf.is_empty() {
        return Err(io::Error::from(ErrorKind::UnexpectedEof));
    }

    let mut ranges_to_keep = vec![];
    let mut pos = 0usize;

    let state = loop {
        // Consume bytes from the current frame
        if remaining_in_frame > 0 {
            let available = buf.len() - pos;
            let consume = available.min(usize::try_from(remaining_in_frame).unwrap_or(usize::MAX));

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

                H3BodyFrameType::Unknown => {
                    // discard
                }

                H3BodyFrameType::Start => unreachable!(),
            }

            pos += consume;
            remaining_in_frame -= consume as u64;

            // If we finished a trailers frame, body is done
            if remaining_in_frame == 0 && frame_type == H3BodyFrameType::Trailers {
                // RFC 9114 §4.1.1: pseudo-headers MUST NOT appear in trailers.
                // We permissively ignore them rather than failing the request.
                *trailers = Some(
                    FieldSection::decode(&self_buffer[..])
                        .map_err(|_| {
                            io::Error::new(ErrorKind::InvalidData, "invalid trailer field section")
                        })?
                        .into_headers()
                        .into_owned(),
                );
                self_buffer.truncate(0);
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
