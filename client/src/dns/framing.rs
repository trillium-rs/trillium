//! The 2-byte length-prefixed framing of RFC 1035, shared by the DoT and DoQ transports (DoQ
//! reuses the same prefix, per RFC 9250). DoH needs none of this — HTTP supplies the message
//! length.

use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::io::{self, ErrorKind};

/// Exchange a single wire-format DNS message over a connected stream: write `len ++ query`, then
/// read `len ++ response`.
///
/// `finish_send` half-closes the write side after the query (`close`) rather than just flushing it.
/// DoQ requires this — each query owns a bidi stream and RFC 9250 mandates a STREAM FIN to mark the
/// query complete; the read side stays open for the response. DoT passes `false`: it keeps
/// the (one-shot) TLS write half open, since closing it would send a TLS `close_notify` and risk
/// tearing the session down before the resolver replies.
pub(super) async fn length_prefixed_exchange<T: AsyncRead + AsyncWrite + Unpin>(
    transport: &mut T,
    query: &[u8],
    finish_send: bool,
) -> io::Result<Vec<u8>> {
    let len = u16::try_from(query.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "DNS query exceeds 65535 bytes"))?;
    let mut framed = Vec::with_capacity(query.len() + 2);
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(query);
    transport.write_all(&framed).await?;
    if finish_send {
        transport.close().await?;
    } else {
        transport.flush().await?;
    }
    log::trace!("length-prefixed exchange: wrote {len}-byte query, awaiting response length");

    let mut len_buf = [0u8; 2];
    transport.read_exact(&mut len_buf).await?;
    let response_len = usize::from(u16::from_be_bytes(len_buf));
    log::trace!("length-prefixed exchange: reading {response_len}-byte response");
    let mut response = vec![0u8; response_len];
    transport.read_exact(&mut response).await?;
    Ok(response)
}

/// Prefix a wire-format DNS message with its 2-byte big-endian length.
///
/// The pipelined DoT path frames and writes queries itself rather than going through
/// [`length_prefixed_exchange`], because responses are demultiplexed by DNS message ID off a
/// shared connection rather than correlated one-per-stream.
pub(super) fn frame(message: &[u8]) -> io::Result<Vec<u8>> {
    let len = u16::try_from(message.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "DNS message exceeds 65535 bytes"))?;
    let mut framed = Vec::with_capacity(message.len() + 2);
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(message);
    Ok(framed)
}

/// Drain one complete length-prefixed message from the front of `buf`, returning the message bytes
/// without the prefix, or `None` if `buf` does not yet hold a full frame. The DoT driver calls this
/// repeatedly as bytes arrive, reassembling messages that may span or share reads.
pub(super) fn take_frame(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let len = usize::from(u16::from_be_bytes([*buf.first()?, *buf.get(1)?]));
    if buf.len() < 2 + len {
        return None;
    }
    let message = buf[2..2 + len].to_vec();
    buf.drain(..2 + len);
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::codec::{build_query, parse_response};
    use futures_lite::future;
    use hickory_proto::{
        op::{Message, MessageType, OpCode},
        rr::{Name, RData, Record, RecordType, rdata::A},
    };
    use std::net::{IpAddr, Ipv4Addr};
    use trillium_testing::{TestTransport, harness, test};

    fn a_response(ip: Ipv4Addr) -> Vec<u8> {
        let mut message = Message::new(0, MessageType::Response, OpCode::Query);
        message.add_answer(Record::from_rdata(
            Name::from_utf8("example.com.").unwrap(),
            60,
            RData::A(A(ip)),
        ));
        message.to_vec().unwrap()
    }

    #[test(harness)]
    async fn length_prefixed_exchange_round_trips() {
        let (mut client, mut server) = TestTransport::new();
        let query = build_query("example.com", 443, RecordType::A).unwrap();
        let ip = Ipv4Addr::new(192, 0, 2, 1);
        let response = a_response(ip);

        // The responder reads the length-prefixed query, asserts it arrived intact, and replies
        // with a length-prefixed response — the resolver half of the DoT/DoQ wire framing.
        let responder = {
            let query = query.clone();
            async move {
                let mut len_buf = [0u8; 2];
                server.read_exact(&mut len_buf).await.unwrap();
                let mut received = vec![0u8; usize::from(u16::from_be_bytes(len_buf))];
                server.read_exact(&mut received).await.unwrap();
                assert_eq!(received, query);

                let mut framed = u16::try_from(response.len())
                    .unwrap()
                    .to_be_bytes()
                    .to_vec();
                framed.extend_from_slice(&response);
                // `TestTransport::write_all` is a synchronous inherent method that appends and
                // wakes the reader — no flush needed.
                server.write_all(&framed);
            }
        };

        let (_, result) = future::zip(
            responder,
            length_prefixed_exchange(&mut client, &query, false),
        )
        .await;

        let (resolved, _) = parse_response(&result.unwrap()).unwrap();
        assert_eq!(resolved.addrs, vec![IpAddr::V4(ip)]);
    }

    #[test]
    fn frame_and_take_frame_round_trip() {
        let mut buf = frame(b"hello").unwrap();
        buf.extend(frame(b"world").unwrap());
        // A trailing partial frame: a length prefix promising more than has arrived.
        buf.extend_from_slice(&[0, 3, b'a']);

        assert_eq!(take_frame(&mut buf).unwrap(), b"hello");
        assert_eq!(take_frame(&mut buf).unwrap(), b"world");
        assert!(take_frame(&mut buf).is_none());

        // Completing the partial frame makes it drainable.
        buf.extend_from_slice(b"bc");
        assert_eq!(take_frame(&mut buf).unwrap(), b"abc");
        assert!(buf.is_empty());
        assert!(take_frame(&mut buf).is_none());
    }
}
