use super::{H2ErrorCode, H2Settings};

pub(crate) mod continuation;
pub(crate) mod data;
pub(crate) mod goaway;
pub(crate) mod headers;
pub(crate) mod ping;
pub(crate) mod priority;
pub(crate) mod rst_stream;
pub(crate) mod settings;
pub(crate) mod window_update;

/// Length of the fixed frame header on the wire (RFC 9113 §4.1).
pub(crate) const FRAME_HEADER_LEN: usize = 9;

/// HTTP/2 frame type identifiers (RFC 9113 §11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FrameType {
    /// §6.1 — carries stream body data.
    Data = 0x0,
    /// §6.2 — opens a stream and carries a header block fragment.
    Headers = 0x1,
    /// §6.3 — deprecated stream priority signal. Parse and discard.
    Priority = 0x2,
    /// §6.4 — abnormally terminates a stream.
    RstStream = 0x3,
    /// §6.5 — conveys connection parameters.
    Settings = 0x4,
    /// §6.6 — initiates a server push. Trillium is server-only so we never send these; receiving
    /// one is a connection error.
    PushPromise = 0x5,
    /// §6.7 — connection-level liveness probe.
    Ping = 0x6,
    /// §6.8 — begins graceful connection shutdown.
    Goaway = 0x7,
    /// §6.9 — advances a flow-control window.
    WindowUpdate = 0x8,
    /// §6.10 — continues an unfinished header block.
    Continuation = 0x9,
}

impl TryFrom<u8> for FrameType {
    type Error = u8;

    /// Unknown frame types return `Err(value)`. Per §5.5 these MUST be ignored.
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
            other => Err(other),
        }
    }
}

// Flag bits (RFC 9113 §6.*). Each is interpreted only on the frame types that define it.
pub(crate) const FLAG_END_STREAM: u8 = 0x01;
pub(crate) const FLAG_ACK: u8 = 0x01;
pub(crate) const FLAG_END_HEADERS: u8 = 0x04;
pub(crate) const FLAG_PADDED: u8 = 0x08;
pub(crate) const FLAG_PRIORITY: u8 = 0x20;

/// A parsed HTTP/2 frame header: length, type, flags, stream id.
///
/// On the wire (§4.1):
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
    /// The caller must ensure `buf.len() >= FRAME_HEADER_LEN`; debug builds check this, release
    /// builds panic on out-of-bounds access.
    ///
    /// Also debug-asserts that `length` fits in 24 bits and `stream_id` fits in 31 bits; release
    /// builds silently truncate (callers should have already enforced `SETTINGS_MAX_FRAME_SIZE`).
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
    /// DATA (§6.1). `data_length` bytes of stream payload follow; then `padding_length` bytes of
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

    /// HEADERS (§6.2). `header_block_length` bytes of header block fragment follow; then
    /// `padding_length` bytes of padding to skip.
    Headers {
        /// Stream identifier.
        stream_id: u32,
        /// Whether `END_STREAM` is set.
        end_stream: bool,
        /// Whether `END_HEADERS` is set.
        end_headers: bool,
        /// Priority block, if `PRIORITY` was set (§6.2). Parsed and reported; the scheme is
        /// deprecated by RFC 9113 §5.3.2.
        priority: Option<PriorityInfo>,
        /// Number of header block bytes the caller should consume from the transport.
        header_block_length: u32,
        /// Number of padding bytes to skip after the header block.
        padding_length: u8,
    },

    /// PRIORITY (§6.3). RFC 9113 §5.3.2 deprecates the scheme, but §5.3.1 still requires
    /// rejecting self-dependency, so the priority block is surfaced to the connection layer.
    Priority {
        /// Stream identifier the priority applies to.
        stream_id: u32,
        /// Priority block (dependency + weight). Not used for scheduling.
        priority: PriorityInfo,
    },

    /// `RST_STREAM` (§6.4).
    RstStream {
        /// Stream identifier being reset.
        stream_id: u32,
        /// Reason the stream was terminated.
        error_code: H2ErrorCode,
    },

    /// SETTINGS (§6.5) with fully decoded parameters.
    Settings(H2Settings),

    /// SETTINGS with the ACK flag set; the payload must be empty.
    SettingsAck,

    /// `PUSH_PROMISE` (§6.6). Trillium rejects these on receipt (server-only implementation with
    /// push disabled). The variant exists so the connection layer can report `PROTOCOL_ERROR`.
    PushPromise {
        /// Stream identifier the promise is announced on.
        stream_id: u32,
        /// Remaining payload bytes after the fixed prefix.
        length: u32,
    },

    /// PING (§6.7).
    Ping {
        /// Opaque 8-byte payload to echo back.
        opaque_data: [u8; 8],
        /// Whether this is an ACK of a previously-sent PING.
        ack: bool,
    },

    /// GOAWAY (§6.8). `debug_data_length` bytes of opaque debug data follow; the caller may read
    /// and log them or discard.
    Goaway {
        /// Highest stream id processed by the peer.
        last_stream_id: u32,
        /// Reason for the shutdown.
        error_code: H2ErrorCode,
        /// Length of the opaque debug-data tail in the frame payload.
        debug_data_length: u32,
    },

    /// `WINDOW_UPDATE` (§6.9). `stream_id == 0` means the connection-level window.
    WindowUpdate {
        /// Stream identifier (0 for the connection-level window).
        stream_id: u32,
        /// Flow-control window increment.
        increment: u32,
    },

    /// CONTINUATION (§6.10). `header_block_length` bytes of header block fragment follow.
    Continuation {
        /// Stream identifier this continuation belongs to.
        stream_id: u32,
        /// Whether `END_HEADERS` is set.
        end_headers: bool,
        /// Number of header block bytes the caller should consume from the transport.
        header_block_length: u32,
    },

    /// An unrecognized frame type (§5.5). The caller skips `length` bytes.
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

/// Stream priority parameters from a HEADERS or PRIORITY frame (§6.3). Deprecated by RFC 9113
/// §5.3.2 — the decoder surfaces them but no enforcement is performed.
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
