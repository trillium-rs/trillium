//! PRIORITY frame (RFC 9113 §6.3). Deprecated (§5.3.2) — parse and discard.

use super::{Frame, FrameDecodeError, FrameHeader, PriorityInfo};
use crate::h2::H2ErrorCode;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if header.stream_id == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    if payload.len() != PriorityInfo::WIRE_LEN as usize {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    // Surface the parsed block — the scheme is deprecated (§5.3.2) so we don't use it for
    // priority decisions, but §5.3.1 requires us to reject self-dependency, which means
    // the connection layer has to see the priority info.
    Ok(Frame::Priority {
        stream_id: header.stream_id,
        priority: PriorityInfo::decode(payload),
    })
}
