//! QPACK encoder stream processing (RFC 9204 §3.2).
//!
//! The encoder stream is a unidirectional stream sent by the peer carrying instructions
//! that modify the dynamic table: Set Dynamic Table Capacity, Insert With Name Reference,
//! Insert With Literal Name, and Duplicate.

use super::{
    ENC_INSTR_INSERT_WITH_LITERAL_NAME, ENC_INSTR_INSERT_WITH_NAME_REF,
    ENC_INSTR_LITERAL_NAME_HUFFMAN_FLAG, ENC_INSTR_NAME_REF_STATIC_FLAG,
    ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY, STRING_HUFFMAN_FLAG,
    dynamic_table::DynamicTable,
    huffman,
    static_table::{StaticHeaderName, static_entry},
};
use crate::{
    HeaderName, HeaderValue,
    h3::{H3Error, H3ErrorCode},
};
use futures_lite::io::{AsyncRead, AsyncReadExt};

/// Process a QPACK encoder stream, applying each instruction to `table`.
///
/// Reads until EOF (clean stream close) or a protocol/I/O error. Intended to run
/// concurrently with header block decoding for the lifetime of the QUIC connection.
pub(crate) async fn process_encoder_stream<T>(
    stream: &mut T,
    table: &DynamicTable,
) -> Result<(), H3Error>
where
    T: AsyncRead + Unpin + Send,
{
    loop {
        let Some(first) = read_first_byte(stream).await? else {
            log::trace!("QPACK encoder stream: EOF");
            return Ok(());
        };

        if first & ENC_INSTR_INSERT_WITH_NAME_REF != 0 {
            // Insert With Name Reference (RFC 9204 §3.2.2): 1Txxxxxx
            let is_static = first & ENC_INSTR_NAME_REF_STATIC_FLAG != 0;
            let name_index = read_varint(first, 6, stream).await?;
            let name: HeaderName<'static> = if is_static {
                let (static_name, _) =
                    static_entry(name_index).map_err(|_| H3ErrorCode::QpackEncoderStreamError)?;
                match static_name {
                    StaticHeaderName::Header(k) => HeaderName::from(*k),
                    StaticHeaderName::Pseudo(p) => HeaderName::from(p.as_str().to_owned()),
                }
            } else {
                table
                    .name_at_relative(name_index)
                    .ok_or(H3ErrorCode::QpackEncoderStreamError)?
            };
            let value = read_string(stream).await?;
            log::trace!(
                "QPACK encoder: Insert With Name Reference [{name}: {}]",
                String::from_utf8_lossy(&value)
            );
            table.insert(name, HeaderValue::from(value))?;
        } else if first & ENC_INSTR_INSERT_WITH_LITERAL_NAME != 0 {
            // Insert With Literal Name (RFC 9204 §3.2.3): 01HXXXxx
            let is_huffman = first & ENC_INSTR_LITERAL_NAME_HUFFMAN_FLAG != 0;
            let name_len = read_varint(first, 5, stream).await?;
            let name_bytes = read_exact(name_len, stream).await?;
            let name_bytes = if is_huffman {
                huffman::decode(&name_bytes).map_err(|_| H3ErrorCode::QpackEncoderStreamError)?
            } else {
                name_bytes
            };
            let name = HeaderName::parse(&name_bytes)
                .map_err(|_| H3ErrorCode::QpackEncoderStreamError)?
                .into_owned();
            let value = read_string(stream).await?;
            log::trace!(
                "QPACK encoder: Insert With Literal Name [{name}: {}]",
                String::from_utf8_lossy(&value)
            );
            table.insert(name, HeaderValue::from(value))?;
        } else if first & ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY != 0 {
            // Set Dynamic Table Capacity (RFC 9204 §3.2.1): 001XXXXX
            let capacity = read_varint(first, 5, stream).await?;
            log::trace!("QPACK encoder: Set Dynamic Table Capacity {capacity}");
            table.set_capacity(capacity)?;
        } else {
            // Duplicate (RFC 9204 §3.2.4): 000XXXXX
            let relative_index = read_varint(first, 5, stream).await?;
            log::trace!("QPACK encoder: Duplicate index {relative_index}");
            table.duplicate(relative_index)?;
        }
    }
}

/// Read the first byte of an encoder instruction, returning `None` on clean EOF.
///
/// Clean EOF is not an error — it means the peer closed the encoder stream (e.g. during
/// QUIC connection teardown). Any mid-instruction EOF is still an error.
async fn read_first_byte(stream: &mut (impl AsyncRead + Unpin)) -> Result<Option<u8>, H3Error> {
    let mut b = [0u8; 1];
    match stream.read(&mut b).await {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(b[0])),
        Err(e) => Err(H3Error::Io(e)),
    }
}

async fn read_byte(stream: &mut (impl AsyncRead + Unpin)) -> Result<u8, H3Error> {
    let mut b = [0u8; 1];
    stream
        .read_exact(&mut b)
        .await
        .map_err(|_| H3ErrorCode::QpackEncoderStreamError)?;
    Ok(b[0])
}

/// Read a QPACK prefix-coded integer where the first byte has already been consumed.
async fn read_varint(
    first: u8,
    prefix_size: u8,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<usize, H3Error> {
    let prefix_mask = u8::MAX >> (8 - prefix_size);
    let mut value = usize::from(first & prefix_mask);
    if value < usize::from(prefix_mask) {
        return Ok(value);
    }
    let mut shift = 0u32;
    loop {
        let byte = read_byte(stream).await?;
        let payload = usize::from(byte & 0x7F);
        let increment = payload
            .checked_shl(shift)
            .ok_or(H3ErrorCode::QpackEncoderStreamError)?;
        value = value
            .checked_add(increment)
            .ok_or(H3ErrorCode::QpackEncoderStreamError)?;
        shift += 7;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
}

async fn read_exact(len: usize, stream: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>, H3Error> {
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|_| H3ErrorCode::QpackEncoderStreamError)?;
    Ok(buf)
}

/// Read a QPACK string literal: H flag + 7-bit length prefix, then the body.
async fn read_string(stream: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>, H3Error> {
    let first = read_byte(stream).await?;
    let is_huffman = first & STRING_HUFFMAN_FLAG != 0;
    let len = read_varint(first, 7, stream).await?;
    let raw = read_exact(len, stream).await?;
    if is_huffman {
        huffman::decode(&raw).map_err(|_| H3ErrorCode::QpackEncoderStreamError.into())
    } else {
        Ok(raw)
    }
}
