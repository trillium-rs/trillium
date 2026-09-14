use super::*;
use crate::{
    HttpConfig, KnownHeaderName,
    h3::Frame,
    headers::qpack::{FieldSection, PseudoHeaders},
};
use encoding_rs::UTF_8;
use futures_lite::{AsyncRead, AsyncReadExt, io::Cursor};

mod frame_decode;
mod received_body;

/// Encode a DATA frame (header + payload) into a Vec.
fn data_frame(payload: &[u8]) -> Vec<u8> {
    let frame = Frame::Data(payload.len() as u64);
    let header_len = frame.encoded_len();
    let mut buf = vec![0u8; header_len + payload.len()];
    frame.encode(&mut buf).unwrap();
    buf[header_len..].copy_from_slice(payload);
    buf
}

/// Encode a HEADERS frame header (no payload — caller appends QPACK bytes).
fn headers_frame(payload_len: u64) -> Vec<u8> {
    let frame = Frame::Headers(payload_len);
    let header_len = frame.encoded_len();
    let mut buf = vec![0u8; header_len];
    frame.encode(&mut buf).unwrap();
    buf
}

/// Encode an unknown frame (type not in the H3 spec) with the given payload.
fn unknown_frame(type_value: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![];
    buf.push(type_value); // 1-byte QUIC varint for type (must be ≤ 0x3F)
    buf.push(payload.len() as u8); // 1-byte QUIC varint for length (must be ≤ 0x3F)
    buf.extend_from_slice(payload);
    buf
}

/// Encode a QUIC varint, appending to `out`.
fn encode_varint(value: u64, out: &mut Vec<u8>) {
    if value < (1 << 6) {
        out.push(value as u8);
    } else if value < (1 << 14) {
        out.push(0x40 | (value >> 8) as u8);
        out.push(value as u8);
    } else if value < (1 << 30) {
        out.extend_from_slice(&[
            0x80 | (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]);
    } else {
        out.extend_from_slice(&[
            0xC0 | (value >> 56) as u8,
            (value >> 48) as u8,
            (value >> 40) as u8,
            (value >> 32) as u8,
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]);
    }
}

/// Encode a GREASE frame with a large (8-byte varint) type value.
/// GREASE type values are of the form `0x1f * N + 0x21`.
fn grease_frame(n: u64, payload: &[u8]) -> Vec<u8> {
    let grease_type = 0x1f * n + 0x21;
    let mut buf = vec![];
    encode_varint(grease_type, &mut buf);
    encode_varint(payload.len() as u64, &mut buf);
    buf.extend_from_slice(payload);
    buf
}
