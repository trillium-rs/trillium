//! Typed parsers and wire-format encoders for QPACK encoder-stream and decoder-stream
//! instructions.
//!
//! The two directions carry different wire vocabularies, so each gets its own enum, parser,
//! and wire encoders ([`encoder::EncoderInstruction`] and [`decoder::DecoderInstruction`]).
//! All wire constants live inside the submodules — callers at the `encoder_dynamic_table` /
//! `decoder_dynamic_table` layer work in terms of the typed instructions and encode helpers,
//! not raw bit patterns.
//!
//! Shared between submodules: the low-level wire-read helpers ([`read_first_byte`],
//! [`read_varint`], [`read_exact`], [`read_string_with_huffman`], [`validate_value`]) and the
//! string encoder ([`encode_string`], also used by the field-section emitter).
//! Read helpers return `Result<_, ReadError>` — the per-direction `parse` function maps
//! [`ReadError::Io`] to `H3Error::Io` and [`ReadError::Violation`] to the appropriate stream
//! error code. Distinguishing the two matters because a connection-lost I/O error during a
//! teardown read should not masquerade as a peer-side QPACK protocol violation.

pub(in crate::headers) mod decoder;
pub(in crate::headers) mod encoder;
pub(in crate::headers) mod field_section;

use crate::headers::{huffman, integer_prefix};
use futures_lite::io::{AsyncRead, AsyncReadExt};
use std::io;

// H flag in a string literal with a 7-bit length prefix.
const STRING_HUFFMAN_FLAG: u8 = 0x80;

/// Failure mode of an instruction-stream read helper.
///
/// `Io` is reserved for connection-teardown I/O errors observed *between* instructions —
/// see [`read_first_byte`]. Any other failure (mid-instruction read, length cap exceeded,
/// huffman decode error, etc.) is a [`ReadError::Violation`] — caller maps it to the
/// stream-specific protocol error code.
#[derive(Debug)]
pub(super) enum ReadError {
    Io(io::Error),
    Violation,
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Read the first byte of an instruction, returning `None` on clean EOF.
///
/// Clean EOF is not an error — the peer closed the stream (e.g. during connection teardown).
/// Surfacing other I/O errors here as [`ReadError::Io`] (rather than `Violation`) lets the
/// caller distinguish "we lost the underlying connection" from "peer sent malformed
/// QPACK"; otherwise every connection-tear-down read would log a bogus protocol violation.
/// Mid-instruction EOF (encountered by any other read helper) is a `Violation`.
pub(super) async fn read_first_byte(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Option<u8>, ReadError> {
    let mut b = [0u8; 1];
    match stream.read(&mut b).await {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(b[0])),
        Err(e) => {
            log::debug!("QPACK: read_first_byte io error: {e}");
            Err(ReadError::Io(e))
        }
    }
}

async fn read_byte(stream: &mut (impl AsyncRead + Unpin)) -> Result<u8, ReadError> {
    let mut b = [0u8; 1];
    stream.read_exact(&mut b).await.map_err(|e| {
        log::error!("QPACK: read_byte io error: {e:?}");
        ReadError::Violation
    })?;
    Ok(b[0])
}

/// Read a QPACK prefix-coded integer whose first byte has already been consumed.
/// `prefix_size` is the number of low bits of `first` occupied by the integer
/// (remaining bits are flags the caller has already extracted).
pub(super) async fn read_varint(
    first: u8,
    prefix_size: u8,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<usize, ReadError> {
    let prefix_mask = u8::MAX >> (8 - prefix_size);
    let mut value = usize::from(first & prefix_mask);
    let mut shift = 0u32;

    if value < usize::from(prefix_mask) {
        return Ok(value);
    }

    loop {
        let byte = read_byte(stream).await?;
        let payload = usize::from(byte & 0x7F);
        let increment = payload.checked_shl(shift).ok_or_else(|| {
            log::error!("QPACK: varint checked_shl overflow (payload={payload}, shift={shift})");
            ReadError::Violation
        })?;

        value = value.checked_add(increment).ok_or_else(|| {
            log::error!(
                "QPACK: varint checked_add overflow (value={value}, increment={increment})"
            );
            ReadError::Violation
        })?;

        shift += 7;

        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
}

/// Read exactly `len` bytes, rejecting `len > max` before any allocation.
///
/// `max` is the caller-supplied ceiling on a single length-prefixed field — for the
/// encoder-stream this is our advertised `SETTINGS_QPACK_MAX_TABLE_CAPACITY`, since any
/// string larger than that would produce an entry the decoder would reject on apply.
/// Bounding before allocation prevents a peer from triggering a multi-gigabyte
/// allocation via a single 10-byte length prefix without delivering any payload.
pub(super) async fn read_exact(
    len: usize,
    max: usize,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Vec<u8>, ReadError> {
    if len > max {
        log::error!("QPACK: read_exact len {len} exceeds max {max}");
        return Err(ReadError::Violation);
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await.map_err(|e| {
        log::error!("QPACK: read_exact({len}) io error: {e:?}");
        ReadError::Violation
    })?;
    Ok(buf)
}

/// Read a QPACK string literal: H flag + 7-bit length prefix, then the body.
/// Huffman-decodes into plain bytes when the H flag is set.
///
/// `max` bounds the raw (on-wire) byte length; see [`read_exact`].
pub(super) async fn read_string_with_huffman(
    max: usize,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Vec<u8>, ReadError> {
    let first = read_byte(stream).await?;
    let is_huffman = first & STRING_HUFFMAN_FLAG != 0;
    let len = read_varint(first, 7, stream).await?;
    let raw = read_exact(len, max, stream).await?;
    if is_huffman {
        huffman::decode(&raw).map_err(|e| {
            log::error!("QPACK: huffman string decode failed ({len} bytes): {e:?}");
            ReadError::Violation
        })
    } else {
        Ok(raw)
    }
}

/// Reject values containing CR, LF, or NUL bytes — RFC 9114 field-value sanitation.
pub(super) fn validate_value(value: &[u8]) -> Result<(), ReadError> {
    if memchr::memchr3(b'\r', b'\n', 0, value).is_some() {
        Err(ReadError::Violation)
    } else {
        Ok(())
    }
}

/// Encode a string literal.
///
/// Tries Huffman encoding and uses it when strictly shorter. The H flag is placed at bit
/// `prefix_size` of the first byte; the length occupies the low `prefix_size` bits as a
/// varint. The caller is responsible for OR-ing any additional flags into the byte at
/// `buf.len()` prior to the call after this returns. Shared between the encoder-stream
/// insert instructions and the field-section emitter.
///
/// No intermediate allocation: the Huffman-vs-raw decision is made from
/// [`huffman::encoded_length_if_shorter`] without materializing the encoded form, and the
/// chosen encoding is written directly into `buf`.
pub(in crate::headers) fn encode_string(value: &[u8], prefix_size: u8, buf: &mut Vec<u8>) {
    let start = buf.len();
    if let Some(huffman_len) = huffman::encoded_length_if_shorter(value) {
        integer_prefix::encode_into(huffman_len, prefix_size, buf);
        buf[start] |= 1_u8 << prefix_size;
        huffman::encode_into(value, buf);
    } else {
        integer_prefix::encode_into(value.len(), prefix_size, buf);
        buf.extend_from_slice(value);
    }
}
