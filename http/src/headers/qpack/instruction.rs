//! Typed parsers and wire-format encoders for QPACK encoder-stream (RFC 9204 §3.2) and
//! decoder-stream (§4.4) instructions.
//!
//! The two directions carry different wire vocabularies, so each gets its own enum, parser,
//! and wire encoders ([`encoder::EncoderInstruction`] and [`decoder::DecoderInstruction`]).
//! All wire constants live inside the submodules — callers at the `encoder_dynamic_table` /
//! `decoder_dynamic_table` layer work in terms of the typed instructions and encode helpers,
//! not raw bit patterns.
//!
//! Shared between submodules: the low-level wire-read helpers ([`read_first_byte`],
//! [`read_varint`], [`read_exact`], [`read_string_with_huffman`], [`validate_value`]) and the
//! §4.1.2 string encoder ([`encode_string`], also used by the §4.5 field-section emitter).
//! Read helpers return `Result<_, ()>` — the per-direction `parse` function maps failure to
//! the appropriate stream error code. Helpers log the original error before discarding it.

pub(in crate::headers) mod decoder;
pub(in crate::headers) mod encoder;
pub(in crate::headers) mod field_section;

use crate::headers::qpack::{huffman, varint};
use futures_lite::io::{AsyncRead, AsyncReadExt};

// §4.1.2: H flag in a string literal with a 7-bit length prefix.
const STRING_HUFFMAN_FLAG: u8 = 0x80;

/// Read the first byte of an instruction, returning `None` on clean EOF.
///
/// Clean EOF is not an error — the peer closed the stream (e.g. during connection teardown).
/// Mid-instruction EOF (encountered by any other read helper) is an error.
pub(super) async fn read_first_byte(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Option<u8>, ()> {
    let mut b = [0u8; 1];
    match stream.read(&mut b).await {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(b[0])),
        Err(e) => {
            log::debug!("QPACK: read_first_byte io error: {e}");
            Err(())
        }
    }
}

async fn read_byte(stream: &mut (impl AsyncRead + Unpin)) -> Result<u8, ()> {
    let mut b = [0u8; 1];
    stream.read_exact(&mut b).await.map_err(|e| {
        log::error!("QPACK: read_byte io error: {e:?}");
    })?;
    Ok(b[0])
}

/// Read a QPACK prefix-coded integer (RFC 9204 §4.1.1) whose first byte has already been
/// consumed. `prefix_size` is the number of low bits of `first` occupied by the integer
/// (remaining bits are flags the caller has already extracted).
pub(super) async fn read_varint(
    first: u8,
    prefix_size: u8,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<usize, ()> {
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
        })?;

        value = value.checked_add(increment).ok_or_else(|| {
            log::error!(
                "QPACK: varint checked_add overflow (value={value}, increment={increment})"
            );
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
/// string larger than that would produce an entry the decoder would reject on apply
/// (RFC 9204 §3.2.2). Bounding before allocation prevents a peer from triggering a
/// multi-gigabyte allocation via a single 10-byte length prefix without delivering any
/// payload.
pub(super) async fn read_exact(
    len: usize,
    max: usize,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Vec<u8>, ()> {
    if len > max {
        log::error!("QPACK: read_exact len {len} exceeds max {max}");
        return Err(());
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await.map_err(|e| {
        log::error!("QPACK: read_exact({len}) io error: {e:?}");
    })?;
    Ok(buf)
}

/// Read a QPACK string literal (RFC 9204 §4.1.2): H flag + 7-bit length prefix, then the
/// body. Huffman-decodes into plain bytes when the H flag is set.
///
/// `max` bounds the raw (on-wire) byte length; see [`read_exact`].
pub(super) async fn read_string_with_huffman(
    max: usize,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Vec<u8>, ()> {
    let first = read_byte(stream).await?;
    let is_huffman = first & STRING_HUFFMAN_FLAG != 0;
    let len = read_varint(first, 7, stream).await?;
    let raw = read_exact(len, max, stream).await?;
    if is_huffman {
        huffman::decode(&raw).map_err(|e| {
            log::error!("QPACK: huffman string decode failed ({len} bytes): {e:?}");
        })
    } else {
        Ok(raw)
    }
}

/// Reject values containing CR, LF, or NUL bytes — RFC 9114 §4.2 field-value sanitation.
pub(super) fn validate_value(value: &[u8]) -> Result<(), ()> {
    if memchr::memchr3(b'\r', b'\n', 0, value).is_some() {
        Err(())
    } else {
        Ok(())
    }
}

/// Encode a string literal per RFC 9204 §4.1.2.
///
/// Tries Huffman encoding and uses it when shorter. The H flag is placed at bit
/// `prefix_size` of the first byte; the length occupies the low `prefix_size` bits as a
/// varint. The caller is responsible for OR-ing any additional flags into the byte at
/// `buf.len()` prior to the call after this returns. Shared between the §3.2 encoder-stream
/// insert instructions and the §4.5 field-section emitter.
pub(in crate::headers) fn encode_string(value: &[u8], prefix_size: u8, buf: &mut Vec<u8>) {
    let huffman_encoded = huffman::encode(value);
    let (bytes, huffman_flag) = if huffman_encoded.len() < value.len() {
        (huffman_encoded.as_slice(), 1_u8 << prefix_size)
    } else {
        (value, 0)
    };
    let start = buf.len();
    buf.extend_from_slice(&varint::encode(bytes.len(), prefix_size));
    buf[start] |= huffman_flag;
    buf.extend_from_slice(bytes);
}
