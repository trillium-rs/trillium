use super::{
    error::H3ErrorCode,
    quic_varint::{self, QuicVarIntError},
    settings::H3Settings,
};

mod stream;
#[cfg(feature = "unstable")]
pub use stream::ActiveFrame;
pub use stream::FrameStream;

#[cfg(test)]
mod tests;

/// H3 frame types per RFC 9114 §7.2.
///
/// Each frame on the wire is: varint(type) + varint(length) + payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FrameType {
    /// §7.2.1 — carries request/response body data.
    Data = 0x00,
    /// §7.2.2 — carries a QPACK-encoded field section (headers or trailers).
    Headers = 0x01,
    /// §7.2.3 — cancels a server push (referenced by push ID).
    CancelPush = 0x03,
    /// §7.2.4 — conveys configuration parameters.
    Settings = 0x04,
    /// §7.2.5 — initiates a server push.
    PushPromise = 0x05,
    /// §7.2.6 — initiates graceful shutdown.
    Goaway = 0x07,
    /// §7.2.7 — controls the number of server pushes.
    MaxPushId = 0x0d,
    /// WebTransport bidi stream signal (draft-ietf-webtrans-http3 §4.2).
    ///
    /// On the wire this looks like a frame header (`varint(0x41) + varint(session_id)`),
    /// but it is not a proper H3 frame — there is no length-delimited payload.
    /// The rest of the stream after the session ID is raw application data.
    WebTransport = 0x41,
}

impl From<FrameType> for u64 {
    fn from(val: FrameType) -> Self {
        val as u64
    }
}

impl TryFrom<u64> for FrameType {
    type Error = u64;

    /// Unrecognized frame types are returned as `Err(value)`.
    /// Per §7.2.8, unknown types MUST be ignored.
    fn try_from(value: u64) -> Result<Self, u64> {
        match value {
            0x00 => Ok(Self::Data),
            0x01 => Ok(Self::Headers),
            0x03 => Ok(Self::CancelPush),
            0x04 => Ok(Self::Settings),
            0x05 => Ok(Self::PushPromise),
            0x07 => Ok(Self::Goaway),
            0x0d => Ok(Self::MaxPushId),
            0x41 => Ok(Self::WebTransport),
            other => {
                log::trace!("did not recognize frame type {value}");
                Err(other)
            }
        }
    }
}

/// A parsed H3 frame header: type + payload length.
///
/// On the wire this is `varint(type) + varint(length)`, followed by
/// `payload_length` bytes of payload (not included here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameHeader {
    /// `None` for unknown frame types (which per §7.2.8 MUST be ignored).
    frame_type: Option<FrameType>,
    /// Length of the frame payload in bytes.
    payload_length: u64,
}

impl FrameHeader {
    /// Try to decode a frame header from the front of `input`.
    /// Returns the header and the number of bytes consumed.
    fn decode(input: &[u8]) -> Result<(Self, usize), FrameDecodeError> {
        let (frame_type, frame_type_bytes) = match quic_varint::decode::<FrameType>(input) {
            Ok((ft, bytes)) => (Some(ft), bytes),
            Err(QuicVarIntError::UnexpectedEnd) => return Err(FrameDecodeError::Incomplete),
            Err(QuicVarIntError::UnknownValue { bytes, .. }) => (None, bytes),
        };

        let (payload_length, payload_length_bytes) =
            quic_varint::decode::<u64>(&input[frame_type_bytes..])
                .map_err(|_| FrameDecodeError::Incomplete)?;

        Ok((
            Self {
                frame_type,
                payload_length,
            },
            frame_type_bytes + payload_length_bytes,
        ))
    }

    /// Encode this frame header into `buf`.
    ///
    /// Does nothing if `frame_type` is `None` (unknown frame types are
    /// never sent, only received and skipped).
    #[cfg(test)]
    fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        let mut written = 0;
        if let Some(ft) = self.frame_type {
            written += quic_varint::encode(ft, buf)?;
            written += quic_varint::encode(self.payload_length, &mut buf[written..])?;
        }
        Some(written)
    }
}

/// Errors from [`Frame::decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecodeError {
    /// Not enough bytes in the input yet.
    Incomplete,
    /// Protocol violation.
    Error(H3ErrorCode),
}

impl From<H3ErrorCode> for FrameDecodeError {
    fn from(code: H3ErrorCode) -> Self {
        Self::Error(code)
    }
}

/// A decoded H3 frame.
///
/// For large-payload frame types (`Data`, `Headers`, `PushPromise`, `Unknown`, and `WebTransport`),
/// only the frame header is consumed; the payload bytes remain in the rest slice for the caller to
/// handle. For control frames (Settings, Goaway, etc.), the payload is fully parsed and consumed.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum Frame {
    /// DATA frame — `payload_length` bytes of body data follow in the rest slice.
    Data(u64),

    /// HEADERS frame — `payload_length` bytes of QPACK-encoded field section follow.
    Headers(u64),

    /// `CANCEL_PUSH` frame — the push ID to cancel.
    CancelPush(u64),

    /// SETTINGS frame — fully parsed connection settings.
    Settings(H3Settings),

    /// `PUSH_PROMISE` frame — push ID, then `field_section_length` bytes of
    /// QPACK-encoded field section follow in the rest slice.
    PushPromise {
        /// The id for this push
        push_id: u64,
        /// header length
        field_section_length: u64,
    },

    /// `GOAWAY` frame — stream or push ID for graceful shutdown.
    Goaway(u64),

    /// `MAX_PUSH_ID` frame — maximum push ID the peer will accept.
    MaxPushId(u64),

    /// WebTransport bidi stream signal — the session ID.
    ///
    /// Unlike other frames, there is no length-delimited payload. The rest of the
    /// stream is raw application data belonging to the WebTransport session.
    WebTransport(u64),

    /// Unknown frame type — `payload_length` bytes to skip follow in the rest slice.
    Unknown(u64),
}

impl Frame {
    /// Decode a frame from the front of `input`.
    ///
    /// Returns the decoded frame and the number of bytes consumed.
    /// For `Data`, `Headers`, `PushPromise`, and `Unknown` frames, only the frame
    /// header is consumed — the payload remains unconsumed.
    /// For control frames the entire frame (header + payload) is consumed.
    ///
    /// # Errors
    ///
    /// Returns a `FrameDecodeError` if we have not read sufficient content or if we encounter a
    /// protocol error
    // Note: on incomplete input this re-parses the frame header from
    // scratch, which is fine — frame headers are at most 16 bytes.
    // Revisit if profiling shows this is hot.
    pub fn decode(input: &[u8]) -> Result<(Self, usize), FrameDecodeError> {
        let mut bytes_read = 0;
        let (header, header_len) = FrameHeader::decode(input)?;
        log::trace!("Decoded frame header {header:?}");
        bytes_read += header_len;

        match header.frame_type {
            // Large-payload frames: return immediately, caller handles payload.
            Some(FrameType::Data) => Ok((Frame::Data(header.payload_length), header_len)),
            Some(FrameType::Headers) => Ok((Frame::Headers(header.payload_length), header_len)),
            // WebTransport: payload_length is actually the session ID.
            // No length-delimited payload — rest of stream is raw application data.
            Some(FrameType::WebTransport) => {
                Ok((Frame::WebTransport(header.payload_length), header_len))
            }
            None => Ok((Frame::Unknown(header.payload_length), header_len)),

            // PushPromise: parse push_id varint from the payload, leave
            // the field section for the caller.
            Some(FrameType::PushPromise) => {
                let (push_id, push_id_len) = quic_varint::decode::<u64>(&input[bytes_read..])
                    .map_err(|_| FrameDecodeError::Incomplete)?;

                bytes_read += push_id_len;

                if push_id_len as u64 > header.payload_length {
                    return Err(H3ErrorCode::FrameError.into());
                }

                Ok((
                    Frame::PushPromise {
                        push_id,
                        field_section_length: header.payload_length - push_id_len as u64,
                    },
                    bytes_read,
                ))
            }

            // Control frames: need full payload buffered.
            Some(FrameType::Settings) => {
                let payload = require_payload(&input[bytes_read..], header.payload_length)?;
                let settings = H3Settings::decode(payload).ok_or(H3ErrorCode::SettingsError)?;
                Ok((Frame::Settings(settings), header_len + payload.len()))
            }

            Some(FrameType::Goaway) => decode_single_varint(
                &input[bytes_read..],
                header_len,
                header.payload_length,
                Frame::Goaway,
            ),

            Some(FrameType::CancelPush) => decode_single_varint(
                &input[bytes_read..],
                header_len,
                header.payload_length,
                Frame::CancelPush,
            ),

            Some(FrameType::MaxPushId) => decode_single_varint(
                &input[bytes_read..],
                header_len,
                header.payload_length,
                Frame::MaxPushId,
            ),
        }
    }

    /// The number of bytes this frame will occupy when encoded.
    ///
    /// For Data, Headers, and `PushPromise` this is only the frame header
    /// (+ `push_id` for `PushPromise`) — the payload is the caller's
    /// responsibility, matching the decode convention.
    /// Returns 0 for Unknown (we never send unknown frames).
    pub fn encoded_len(&self) -> usize {
        match self {
            Frame::Data(len) => frame_header_len(FrameType::Data, *len),
            Frame::Headers(len) => frame_header_len(FrameType::Headers, *len),

            Frame::CancelPush(id) => single_varint_frame_len(FrameType::CancelPush, *id),
            Frame::Goaway(id) => single_varint_frame_len(FrameType::Goaway, *id),
            Frame::MaxPushId(id) => single_varint_frame_len(FrameType::MaxPushId, *id),

            Frame::Settings(settings) => {
                let payload_len = settings.encoded_len();
                frame_header_len(FrameType::Settings, payload_len as u64) + payload_len
            }

            Frame::PushPromise {
                push_id,
                field_section_length,
            } => {
                let push_id_len = quic_varint::encoded_len(*push_id);
                let payload_len = push_id_len as u64 + field_section_length;
                frame_header_len(FrameType::PushPromise, payload_len) + push_id_len
            }

            // varint(0x41) + varint(session_id), no length field
            Frame::WebTransport(session_id) => {
                quic_varint::encoded_len(FrameType::WebTransport)
                    + quic_varint::encoded_len(*session_id)
            }

            Frame::Unknown(_) => 0,
        }
    }

    /// Encode this frame into `buf`.
    ///
    /// Returns `None` if `buf` is too small (check [`encoded_len`](Self::encoded_len)
    /// first). Returns `Some(bytes_written)` on success.
    ///
    /// For Data, Headers, and `PushPromise`, only the frame header is written
    /// (+ `push_id` for `PushPromise`). The caller writes the payload afterward.
    /// For Unknown, returns `Some(0)` (nothing to send).
    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        let len = self.encoded_len();
        if buf.len() < len {
            return None;
        }
        match self {
            Frame::Data(payload_len) => encode_frame_header(FrameType::Data, *payload_len, buf),

            Frame::Headers(payload_len) => {
                encode_frame_header(FrameType::Headers, *payload_len, buf)
            }

            Frame::CancelPush(id) => encode_single_varint_frame(FrameType::CancelPush, *id, buf),

            Frame::Goaway(id) => encode_single_varint_frame(FrameType::Goaway, *id, buf),

            Frame::MaxPushId(id) => encode_single_varint_frame(FrameType::MaxPushId, *id, buf),

            Frame::Settings(settings) => {
                let mut written = 0;
                let payload_len = settings.encoded_len() as u64;
                written +=
                    encode_frame_header(FrameType::Settings, payload_len, &mut buf[written..])?;
                written += settings.encode(&mut buf[written..])?;
                Some(written)
            }

            Frame::PushPromise {
                push_id,
                field_section_length,
            } => {
                let mut written = 0;
                let push_id_len = quic_varint::encoded_len(*push_id) as u64;
                let payload_length = push_id_len + field_section_length;
                written += encode_frame_header(
                    FrameType::PushPromise,
                    payload_length,
                    &mut buf[written..],
                )?;
                written += quic_varint::encode(*push_id, &mut buf[written..])?;
                Some(written)
            }
            Frame::WebTransport(session_id) => {
                let mut written = quic_varint::encode(FrameType::WebTransport, buf)?;
                written += quic_varint::encode(*session_id, &mut buf[written..])?;
                Some(written)
            }

            Frame::Unknown(_) => Some(0),
        }
    }
}

/// Size of a frame header (type varint + length varint) on the wire.
fn frame_header_len(frame_type: FrameType, payload_length: u64) -> usize {
    quic_varint::encoded_len(frame_type) + quic_varint::encoded_len(payload_length)
}

/// Total encoded size of a frame whose payload is a single varint.
fn single_varint_frame_len(frame_type: FrameType, value: u64) -> usize {
    let payload_len = quic_varint::encoded_len(value);
    frame_header_len(frame_type, payload_len as u64) + payload_len
}

/// Write a frame header (type + length) into `buf`. Returns bytes written.
fn encode_frame_header(
    frame_type: FrameType,
    payload_length: u64,
    buf: &mut [u8],
) -> Option<usize> {
    let mut written = 0;
    written += quic_varint::encode(frame_type, &mut buf[written..])?;
    written += quic_varint::encode(payload_length, &mut buf[written..])?;
    Some(written)
}

/// Write a complete single-varint-payload frame into `buf`. Returns bytes written.
fn encode_single_varint_frame(frame_type: FrameType, value: u64, buf: &mut [u8]) -> Option<usize> {
    let payload_len = quic_varint::encoded_len(value) as u64;
    let mut written = encode_frame_header(frame_type, payload_len, buf)?;
    written += quic_varint::encode(value, &mut buf[written..])?;
    Some(written)
}

/// Check that `after_header` contains at least `payload_length` bytes,
/// returning the payload slice.
fn require_payload(after_header: &[u8], payload_length: u64) -> Result<&[u8], FrameDecodeError> {
    let len = usize::try_from(payload_length).map_err(|_| H3ErrorCode::FrameError)?;
    if after_header.len() < len {
        Err(FrameDecodeError::Incomplete)
    } else {
        Ok(&after_header[..len])
    }
}

/// Decode a frame whose payload is exactly one varint.
/// Returns `Err(Incomplete)` if not enough bytes,
/// `Err(Error(FrameError))` if the varint doesn't consume
/// exactly `payload_length` bytes.
fn decode_single_varint(
    after_header: &[u8],
    header_len: usize,
    payload_length: u64,
    wrap: fn(u64) -> Frame,
) -> Result<(Frame, usize), FrameDecodeError> {
    let payload = require_payload(after_header, payload_length)?;
    let (value, bytes_read) =
        quic_varint::decode::<u64>(payload).map_err(|_| H3ErrorCode::FrameError)?;
    if bytes_read != after_header.len() {
        return Err(H3ErrorCode::FrameError.into());
    }
    Ok((wrap(value), header_len + payload.len()))
}

/// Unidirectional stream types per RFC 9114 §6.2 and RFC 9204 §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum UniStreamType {
    /// H3 control stream (§6.2.1).
    Control = 0x00,
    /// Server push stream (§6.2.2).
    Push = 0x01,
    /// QPACK encoder stream (RFC 9204 §4.2).
    QpackEncoder = 0x02,
    /// QPACK decoder stream (RFC 9204 §4.2).
    QpackDecoder = 0x03,
    /// WebTransport unidirectional stream (draft-ietf-webtrans-http3 §4.1).
    WebTransport = 0x54,
}

impl From<UniStreamType> for u64 {
    fn from(value: UniStreamType) -> Self {
        value as u64
    }
}

impl TryFrom<u64> for UniStreamType {
    type Error = u64;

    /// Unrecognized stream types are returned as `Err(value)`.
    /// Per §6.2, unknown types MUST be ignored.
    fn try_from(value: u64) -> Result<Self, u64> {
        match value {
            0x00 => Ok(Self::Control),
            0x01 => Ok(Self::Push),
            0x02 => Ok(Self::QpackEncoder),
            0x03 => Ok(Self::QpackDecoder),
            0x54 => Ok(Self::WebTransport),
            other => Err(other),
        }
    }
}
