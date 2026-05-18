//! Typed parser and wire-format encoders for QPACK decoder-stream instructions.
//!
//! [`parse`] reads one instruction off the wire and returns it as a [`DecoderInstruction`]
//! without applying it to any table. The consumer ([`encoder_dynamic_table::EncoderDynamicTable`])
//! dispatches the parsed value to update its bookkeeping.
//!
//! The `encode_*` functions are the symmetric wire encoders. They are used by
//! [`decoder_dynamic_table::DecoderDynamicTable`]'s writer task to signal Section
//! Acknowledgement and Insert Count Increment back to the peer.
//!
//! [`encoder_dynamic_table::EncoderDynamicTable`]: crate::headers::qpack::encoder_dynamic_table::EncoderDynamicTable
//! [`decoder_dynamic_table::DecoderDynamicTable`]: crate::headers::qpack::decoder_dynamic_table::DecoderDynamicTable

use super::{ReadError, read_first_byte, read_varint};
use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::integer_prefix,
};
use futures_lite::io::AsyncRead;

// Section Acknowledgement — first byte pattern 1xxxxxxx with 7-bit prefix stream ID.
const SECTION_ACK: u8 = 0x80;
// Stream Cancellation — first byte pattern 01xxxxxx with 6-bit prefix stream ID.
const STREAM_CANCEL: u8 = 0x40;
// Insert Count Increment — first byte pattern 00xxxxxx with 6-bit prefix increment.
// High bits are zero, so the constant is just documentation for the encode path (no OR-in
// needed).
const INSERT_COUNT_INC: u8 = 0x00;

/// One parsed decoder-stream instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::headers) enum DecoderInstruction {
    /// Section Acknowledgement.
    SectionAcknowledgement { stream_id: u64 },
    /// Stream Cancellation.
    StreamCancellation { stream_id: u64 },
    /// Insert Count Increment.
    InsertCountIncrement { increment: u64 },
}

/// Parse the next decoder-stream instruction from `stream`.
///
/// Returns `Ok(None)` on clean EOF between instructions. `Ok(Some(_))` is a parsed
/// instruction; `Err(H3Error::Io)` propagates an underlying transport I/O error
/// (e.g. connection lost while we were polling for the next instruction);
/// `Err(H3Error::Protocol(QpackDecoderStreamError))` is a wire-format violation by
/// the peer.
pub(in crate::headers) async fn parse(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Option<DecoderInstruction>, H3Error> {
    parse_inner(stream).await.map_err(|e| match e {
        ReadError::Io(io) => H3Error::Io(io),
        ReadError::Violation => H3ErrorCode::QpackDecoderStreamError.into(),
    })
}

async fn parse_inner(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Option<DecoderInstruction>, ReadError> {
    let Some(first) = read_first_byte(stream).await? else {
        return Ok(None);
    };

    let instr = if first & SECTION_ACK != 0 {
        let stream_id = read_varint(first, 7, stream).await? as u64;
        DecoderInstruction::SectionAcknowledgement { stream_id }
    } else if first & STREAM_CANCEL != 0 {
        let stream_id = read_varint(first, 6, stream).await? as u64;
        DecoderInstruction::StreamCancellation { stream_id }
    } else {
        let increment = read_varint(first, 6, stream).await? as u64;
        DecoderInstruction::InsertCountIncrement { increment }
    };

    Ok(Some(instr))
}

// --- wire encoders ---

/// Section Acknowledgement: `1XXXXXXX` with a 7-bit prefix integer for the stream ID.
pub(in crate::headers) fn encode_section_ack(stream_id: u64, buf: &mut Vec<u8>) {
    let start = buf.len();
    integer_prefix::encode_into(usize::try_from(stream_id).unwrap_or(usize::MAX), 7, buf);
    buf[start] |= SECTION_ACK;
}

/// Insert Count Increment: `00XXXXXX` with a 6-bit prefix integer for the increment.
pub(in crate::headers) fn encode_insert_count_increment(increment: u64, buf: &mut Vec<u8>) {
    let start = buf.len();
    integer_prefix::encode_into(usize::try_from(increment).unwrap_or(usize::MAX), 6, buf);
    buf[start] |= INSERT_COUNT_INC; // 0x00 — no-op, but makes the intent explicit
}
