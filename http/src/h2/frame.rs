use super::{H2ErrorCode, H2Settings};
use crate::Priority;

pub(crate) mod continuation;
pub(crate) mod data;
pub(crate) mod goaway;
pub(crate) mod headers;
pub(crate) mod ping;
pub(crate) mod priority;
pub(crate) mod priority_update;
pub(crate) mod rst_stream;
pub(crate) mod settings;
pub(crate) mod window_update;

/// Length of the fixed frame header on the wire.
pub(crate) const FRAME_HEADER_LEN: usize = 9;

/// HTTP/2 frame type identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FrameType {
    /// Carries stream body data.
    Data = 0x0,
    /// Opens a stream and carries a header block fragment.
    Headers = 0x1,
    /// Deprecated stream priority signal. Parse and discard.
    Priority = 0x2,
    /// Abnormally terminates a stream.
    RstStream = 0x3,
    /// Conveys connection parameters.
    Settings = 0x4,
    /// Initiates a server push. Receiving one is a connection error here — server push is
    /// never sent, so the variant exists only for the connection layer to reject it.
    PushPromise = 0x5,
    /// Connection-level liveness probe.
    Ping = 0x6,
    /// Begins graceful connection shutdown.
    Goaway = 0x7,
    /// Advances a flow-control window.
    WindowUpdate = 0x8,
    /// Continues an unfinished header block.
    Continuation = 0x9,
    /// Reprioritizes a request stream (RFC 9218).
    PriorityUpdate = 0x10,
}

impl TryFrom<u8> for FrameType {
    type Error = u8;

    /// Unknown frame types return `Err(value)`; the caller ignores them.
    fn try_from(value: u8) -> Result<Self, u8> {
        match value {
            0x0 => Ok(Self::Data),
            0x1 => Ok(Self::Headers),
            0x2 => Ok(Self::Priority),
            0x3 => Ok(Self::RstStream),
            0x4 => Ok(Self::Settings),
            0x5 => Ok(Self::PushPromise),
            0x6 => Ok(Self::Ping),
            0x7 => Ok(Self::Goaway),
            0x8 => Ok(Self::WindowUpdate),
            0x9 => Ok(Self::Continuation),
            0x10 => Ok(Self::PriorityUpdate),
            other => Err(other),
        }
    }
}

// Flag bits. Each is interpreted only on the frame types that define it.
pub(crate) const FLAG_END_STREAM: u8 = 0x01;
pub(crate) const FLAG_ACK: u8 = 0x01;
pub(crate) const FLAG_END_HEADERS: u8 = 0x04;
pub(crate) const FLAG_PADDED: u8 = 0x08;
pub(crate) const FLAG_PRIORITY: u8 = 0x20;

/// A parsed HTTP/2 frame header: length, type, flags, stream id.
///
/// On the wire:
/// ```text
/// length:24 | type:8 | flags:8 | R:1 + stream_id:31
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameHeader {
    /// Payload length in bytes (24-bit).
    pub(crate) length: u32,
    /// Raw type byte. Compare against `FrameType::try_from`.
    pub(crate) frame_type: u8,
    /// Frame-type-specific flags.
    pub(crate) flags: u8,
    /// Stream identifier (31-bit; the reserved high bit is masked off).
    pub(crate) stream_id: u32,
}

impl FrameHeader {
    /// Decode the 9-byte frame header. Returns `None` if `input` is too short.
    pub(crate) fn decode(input: &[u8]) -> Option<Self> {
        if input.len() < FRAME_HEADER_LEN {
            return None;
        }
        let length = u32::from_be_bytes([0, input[0], input[1], input[2]]);
        let frame_type = input[3];
        let flags = input[4];
        let stream_id = u32::from_be_bytes([input[5], input[6], input[7], input[8]]) & 0x7FFF_FFFF;
        Some(Self {
            length,
            frame_type,
            flags,
            stream_id,
        })
    }

    /// Encode the 9-byte frame header into the first [`FRAME_HEADER_LEN`] bytes of `buf`.
    ///
    /// Debug-asserts that `buf.len() >= FRAME_HEADER_LEN`, `length` fits in 24 bits, and
    /// `stream_id` fits in 31 bits. In release, an out-of-range `length` silently encodes
    /// only its low 24 bits — callers are expected to have bounded `length` against
    /// `SETTINGS_MAX_FRAME_SIZE` (≤ 2^24 − 1) and `stream_id` against the 31-bit range
    /// upstream.
    pub(crate) fn encode(&self, buf: &mut [u8]) {
        debug_assert!(
            buf.len() >= FRAME_HEADER_LEN,
            "frame header buffer too small"
        );
        debug_assert!(self.length < (1 << 24), "payload length exceeds 24 bits");
        debug_assert!(self.stream_id < (1 << 31), "stream id exceeds 31 bits");
        let length = self.length.to_be_bytes();
        buf[0] = length[1];
        buf[1] = length[2];
        buf[2] = length[3];
        buf[3] = self.frame_type;
        buf[4] = self.flags;
        buf[5..9].copy_from_slice(&(self.stream_id & 0x7FFF_FFFF).to_be_bytes());
    }
}

/// Errors from [`Frame::decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameDecodeError {
    /// Not enough bytes in the input yet.
    Incomplete,
    /// Protocol violation. Map to `GOAWAY` or `RST_STREAM` based on context.
    Error(H2ErrorCode),
}

impl From<H2ErrorCode> for FrameDecodeError {
    fn from(code: H2ErrorCode) -> Self {
        Self::Error(code)
    }
}

/// A decoded HTTP/2 frame.
///
/// Frames carrying a header block or data body (Data, Headers, Continuation, `PushPromise`,
/// Unknown) report their post-prefix payload length for the caller to consume from the transport;
/// the fixed header (and any PADDED / PRIORITY prefix) is the only portion [`Frame::decode`]
/// consumes from the input slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frame {
    /// DATA. `data_length` bytes of stream payload follow; then `padding_length` bytes of
    /// padding to skip.
    Data {
        /// Stream identifier.
        stream_id: u32,
        /// Whether `END_STREAM` is set.
        end_stream: bool,
        /// Number of data bytes the caller should consume from the transport.
        data_length: u32,
        /// Number of padding bytes to skip after the data.
        padding_length: u8,
    },

    /// HEADERS. `header_block_length` bytes of header block fragment follow; then
    /// `padding_length` bytes of padding to skip.
    Headers {
        /// Stream identifier.
        stream_id: u32,
        /// Whether `END_STREAM` is set.
        end_stream: bool,
        /// Whether `END_HEADERS` is set.
        end_headers: bool,
        /// Priority block, if `PRIORITY` was set. Parsed and reported; the scheme is
        /// deprecated by RFC 9113.
        priority: Option<PriorityInfo>,
        /// Number of header block bytes the caller should consume from the transport.
        header_block_length: u32,
        /// Number of padding bytes to skip after the header block.
        padding_length: u8,
    },

    /// PRIORITY. RFC 9113 deprecates the priority scheme, but self-dependency rejection still
    /// applies, so the priority block is surfaced to the connection layer.
    Priority {
        /// Stream identifier the priority applies to.
        stream_id: u32,
        /// Priority block (dependency + weight). Not used for scheduling.
        priority: PriorityInfo,
    },

    /// `RST_STREAM`.
    RstStream {
        /// Stream identifier being reset.
        stream_id: u32,
        /// Reason the stream was terminated.
        error_code: H2ErrorCode,
    },

    /// SETTINGS with fully decoded parameters.
    Settings(H2Settings),

    /// SETTINGS with the ACK flag set; the payload must be empty.
    SettingsAck,

    /// `PUSH_PROMISE`. Surfaced for rejection: server push is disabled, so the connection
    /// layer responds with `PROTOCOL_ERROR`.
    PushPromise {
        /// Stream identifier the promise is announced on.
        stream_id: u32,
        /// Remaining payload bytes after the fixed prefix.
        length: u32,
    },

    /// PING.
    Ping {
        /// Opaque 8-byte payload to echo back.
        opaque_data: [u8; 8],
        /// Whether this is an ACK of a previously-sent PING.
        ack: bool,
    },

    /// GOAWAY. `debug_data_length` bytes of opaque debug data follow; the caller may read
    /// and log them or discard.
    Goaway {
        /// Highest stream id processed by the peer.
        last_stream_id: u32,
        /// Reason for the shutdown.
        error_code: H2ErrorCode,
        /// Length of the opaque debug-data tail in the frame payload.
        debug_data_length: u32,
    },

    /// `WINDOW_UPDATE`. `stream_id == 0` means the connection-level window.
    WindowUpdate {
        /// Stream identifier (0 for the connection-level window).
        stream_id: u32,
        /// Flow-control window increment.
        increment: u32,
    },

    /// CONTINUATION. `header_block_length` bytes of header block fragment follow.
    Continuation {
        /// Stream identifier this continuation belongs to.
        stream_id: u32,
        /// Whether `END_HEADERS` is set.
        end_headers: bool,
        /// Number of header block bytes the caller should consume from the transport.
        header_block_length: u32,
    },

    /// `PRIORITY_UPDATE` (RFC 9218). Reprioritizes the request stream identified by
    /// `prioritized_stream_id`. Carried on the connection control stream.
    PriorityUpdate {
        /// The request stream this priority applies to.
        prioritized_stream_id: u32,
        /// The signaled priority (defaults substituted for any malformed field).
        priority: Priority,
    },

    /// An unrecognized frame type. The caller skips `length` bytes.
    Unknown {
        /// Stream identifier.
        stream_id: u32,
        /// Raw frame type byte.
        frame_type: u8,
        /// Frame-type-specific flags.
        flags: u8,
        /// Bytes of payload to skip.
        length: u32,
    },
}

/// Stream priority parameters from a HEADERS or PRIORITY frame. Deprecated by RFC 9113 —
/// the decoder surfaces them but no enforcement is performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PriorityInfo {
    /// Whether the dependency is exclusive.
    pub(crate) exclusive: bool,
    /// Stream id this stream depends on.
    pub(crate) stream_dependency: u32,
    /// Weight, 1..=256 (encoded as weight-1 on the wire).
    pub(crate) weight: u16,
}

impl PriorityInfo {
    /// The size of the priority block on the wire.
    pub(crate) const WIRE_LEN: u32 = 5;

    pub(crate) fn decode(input: &[u8]) -> Self {
        debug_assert!(input.len() >= Self::WIRE_LEN as usize);
        let dep_word = u32::from_be_bytes([input[0], input[1], input[2], input[3]]);
        Self {
            exclusive: dep_word & 0x8000_0000 != 0,
            stream_dependency: dep_word & 0x7FFF_FFFF,
            weight: u16::from(input[4]) + 1,
        }
    }
}

impl Frame {
    /// Decode a single frame from the front of `input`.
    ///
    /// Returns `(Frame, consumed_bytes)`. For large-payload frame types (Data, Headers,
    /// Continuation, `PushPromise`, Unknown), `consumed_bytes` covers only the 9-byte frame header
    /// plus any per-type fixed prefix (pad length byte, priority block, promised stream id); the
    /// payload itself remains unconsumed in `input` for the caller to stream.
    ///
    /// For control frames (Settings, Ping, `RstStream`, Goaway, `WindowUpdate`, Priority) the
    /// entire frame is consumed.
    ///
    /// # Errors
    ///
    /// `FrameDecodeError::Incomplete` if `input` doesn't yet contain the whole frame header plus
    /// the fixed control-frame payload. `FrameDecodeError::Error(code)` for any protocol violation
    /// detected during decoding.
    pub(crate) fn decode(input: &[u8]) -> Result<(Self, usize), FrameDecodeError> {
        let header = FrameHeader::decode(input).ok_or(FrameDecodeError::Incomplete)?;
        let prefix_input = || {
            input
                .get(FRAME_HEADER_LEN..)
                .ok_or(FrameDecodeError::Incomplete)
        };
        match FrameType::try_from(header.frame_type) {
            Ok(FrameType::Data) => {
                let (frame, prefix_consumed) = data::decode_prefix(header, prefix_input()?)?;
                Ok((frame, FRAME_HEADER_LEN + prefix_consumed))
            }
            Ok(FrameType::Headers) => {
                let (frame, prefix_consumed) = headers::decode_prefix(header, prefix_input()?)?;
                Ok((frame, FRAME_HEADER_LEN + prefix_consumed))
            }
            Ok(FrameType::Continuation) => {
                continuation::decode(header).map(|f| (f, FRAME_HEADER_LEN))
            }
            Ok(FrameType::PushPromise) => Ok((
                Frame::PushPromise {
                    stream_id: header.stream_id,
                    length: header.length,
                },
                FRAME_HEADER_LEN,
            )),
            Ok(FrameType::Priority) => {
                let payload = require_payload(input, header)?;
                priority::decode(header, payload).map(|f| (f, FRAME_HEADER_LEN + payload.len()))
            }
            Ok(FrameType::RstStream) => {
                let payload = require_payload(input, header)?;
                rst_stream::decode(header, payload).map(|f| (f, FRAME_HEADER_LEN + payload.len()))
            }
            Ok(FrameType::Settings) => {
                let payload = require_payload(input, header)?;
                settings::decode(header, payload).map(|f| (f, FRAME_HEADER_LEN + payload.len()))
            }
            Ok(FrameType::Ping) => {
                let payload = require_payload(input, header)?;
                ping::decode(header, payload).map(|f| (f, FRAME_HEADER_LEN + payload.len()))
            }
            Ok(FrameType::Goaway) => {
                let payload = require_payload(input, header)?;
                goaway::decode(header, payload).map(|f| (f, FRAME_HEADER_LEN + payload.len()))
            }
            Ok(FrameType::WindowUpdate) => {
                let payload = require_payload(input, header)?;
                window_update::decode(header, payload)
                    .map(|f| (f, FRAME_HEADER_LEN + payload.len()))
            }
            Ok(FrameType::PriorityUpdate) => {
                let payload = require_payload(input, header)?;
                priority_update::decode(header, payload)
                    .map(|f| (f, FRAME_HEADER_LEN + payload.len()))
            }
            Err(frame_type) => Ok((
                Frame::Unknown {
                    stream_id: header.stream_id,
                    frame_type,
                    flags: header.flags,
                    length: header.length,
                },
                FRAME_HEADER_LEN,
            )),
        }
    }
}

pub(crate) fn require_payload(
    input: &[u8],
    header: FrameHeader,
) -> Result<&[u8], FrameDecodeError> {
    let length = usize::try_from(header.length).map_err(|_| H2ErrorCode::FrameSizeError)?;
    input
        .get(FRAME_HEADER_LEN..FRAME_HEADER_LEN + length)
        .ok_or(FrameDecodeError::Incomplete)
}

/// Test helper: build a complete h2 frame (9-byte header + payload) from explicit
/// field values.
#[cfg(test)]
pub(crate) fn encode_frame(
    frame_type: FrameType,
    flags: u8,
    stream_id: u32,
    payload: &[u8],
) -> Vec<u8> {
    let mut buf = vec![0u8; FRAME_HEADER_LEN + payload.len()];
    FrameHeader {
        length: u32::try_from(payload.len()).unwrap(),
        frame_type: frame_type as u8,
        flags,
        stream_id,
    }
    .encode(&mut buf);
    buf[FRAME_HEADER_LEN..].copy_from_slice(payload);
    buf
}

#[cfg(test)]
mod tests {
    #![allow(clippy::cast_possible_truncation)] // fixed-size test payloads

    use super::*;

    #[test]
    fn frame_header_roundtrip() {
        let header = FrameHeader {
            length: 0x00_01_02_03 & 0x00FF_FFFF,
            frame_type: 0x09,
            flags: 0x0F,
            stream_id: 0x1234_5678,
        };
        let mut buf = [0u8; FRAME_HEADER_LEN];
        header.encode(&mut buf);
        let decoded = FrameHeader::decode(&buf).unwrap();
        assert_eq!(decoded, header);
    }

    #[test]
    fn frame_header_masks_reserved_bit_on_decode() {
        let mut buf = [0u8; FRAME_HEADER_LEN];
        // length=1, type=6 (PING), flags=0, stream_id with reserved bit set
        buf[2] = 1;
        buf[3] = 0x06;
        buf[5..9].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        let decoded = FrameHeader::decode(&buf).unwrap();
        assert_eq!(decoded.stream_id, 0x7FFF_FFFF);
    }

    #[test]
    fn frame_header_incomplete() {
        assert!(FrameHeader::decode(&[0u8; FRAME_HEADER_LEN - 1]).is_none());
    }

    #[test]
    fn unknown_frame_type_returns_unknown_variant() {
        let payload = [1u8, 2, 3];
        let mut buf = vec![0u8; FRAME_HEADER_LEN + payload.len()];
        FrameHeader {
            length: u32::try_from(payload.len()).unwrap(),
            frame_type: 0xBE,
            flags: 0xEF,
            stream_id: 5,
        }
        .encode((&mut buf[..FRAME_HEADER_LEN]).try_into().unwrap());
        buf[FRAME_HEADER_LEN..].copy_from_slice(&payload);

        let (frame, consumed) = Frame::decode(&buf).unwrap();
        // Only the header is consumed; payload stays in the slice for the caller to skip.
        assert_eq!(consumed, FRAME_HEADER_LEN);
        assert_eq!(
            frame,
            Frame::Unknown {
                stream_id: 5,
                frame_type: 0xBE,
                flags: 0xEF,
                length: 3,
            }
        );
    }

    #[test]
    fn push_promise_variant_surfaced_for_rejection() {
        let payload = [0u8; 8]; // promised_stream_id + header fragment
        let mut buf = vec![0u8; FRAME_HEADER_LEN + payload.len()];
        FrameHeader {
            length: u32::try_from(payload.len()).unwrap(),
            frame_type: FrameType::PushPromise as u8,
            flags: 0,
            stream_id: 1,
        }
        .encode((&mut buf[..FRAME_HEADER_LEN]).try_into().unwrap());
        buf[FRAME_HEADER_LEN..].copy_from_slice(&payload);

        let (frame, consumed) = Frame::decode(&buf).unwrap();
        assert_eq!(consumed, FRAME_HEADER_LEN);
        assert_eq!(
            frame,
            Frame::PushPromise {
                stream_id: 1,
                length: 8,
            }
        );
    }

    #[test]
    fn incomplete_header_is_incomplete() {
        assert_eq!(Frame::decode(&[0u8; 4]), Err(FrameDecodeError::Incomplete));
    }

    #[test]
    fn incomplete_control_payload_is_incomplete() {
        // PING frame declares length=8 but we only provide 4 payload bytes.
        let mut buf = vec![0u8; FRAME_HEADER_LEN + 4];
        FrameHeader {
            length: 8,
            frame_type: FrameType::Ping as u8,
            flags: 0,
            stream_id: 0,
        }
        .encode((&mut buf[..FRAME_HEADER_LEN]).try_into().unwrap());
        assert_eq!(Frame::decode(&buf), Err(FrameDecodeError::Incomplete));
    }
}
