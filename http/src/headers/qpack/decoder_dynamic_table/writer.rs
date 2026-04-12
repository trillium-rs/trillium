use crate::{
    h3::{H3Error, MAX_BUFFER_SIZE, UniStreamType, quic_varint},
    headers::qpack::{
        DEC_INSTR_INSERT_COUNT_INC, DEC_INSTR_SECTION_ACK, DecoderDynamicTable, varint,
    },
};
use futures_lite::{AsyncWrite, AsyncWriteExt};
use std::io::{self, ErrorKind};
use swansong::Swansong;

impl DecoderDynamicTable {
    pub(crate) async fn run_writer<T: AsyncWrite + Unpin + Send>(
        &self,
        stream: &mut T,
        swansong: Swansong,
    ) -> Result<(), H3Error> {
        let mut buf = vec![0; 64];
        write(&mut buf, &mut *stream, |buf| {
            quic_varint::encode(UniStreamType::QpackDecoder, buf)
        })
        .await?;

        log::trace!("QPACK decoder stream: started");
        let mut last_reported_insert_count = 0u64;

        loop {
            let listener = self.listen();
            let (pending_acks, insert_count) = self.drain_pending_acks_and_count();

            let mut instructions = Vec::new();
            for ack in pending_acks {
                log::trace!(
                    "QPACK decoder: Section Acknowledgement for stream {}",
                    ack.stream_id
                );
                encode_section_ack(ack.stream_id, &mut instructions);
                // A Section Acknowledgement implicitly tells the encoder KRC >=
                // required_insert_count, so those inserts must not also be counted
                // in ICI (RFC 9204 §4.4.3).
                last_reported_insert_count =
                    last_reported_insert_count.max(ack.required_insert_count);
            }
            let increment = insert_count - last_reported_insert_count;
            if increment > 0 {
                log::trace!(
                    "QPACK decoder: Insert Count Increment {increment} (total {insert_count})"
                );
                encode_insert_count_increment(increment, &mut instructions);
                last_reported_insert_count = insert_count;
            }
            if !instructions.is_empty() {
                log::trace!(
                    "QPACK decoder: writing {} instruction bytes",
                    instructions.len()
                );
                stream.write_all(&instructions).await?;
                stream.flush().await?;
                log::trace!("QPACK decoder: flush complete");
            }

            log::trace!("QPACK decoder: waiting for table event or shutdown");
            let shutdown = futures_lite::future::or(
                async {
                    listener.await;
                    log::trace!("QPACK decoder: table event received");
                    false
                },
                async {
                    swansong.clone().await;
                    true
                },
            )
            .await;
            if shutdown {
                break Ok(());
            }
        }
    }
}
// --- QPACK decoder stream helpers ---

/// Encode a Section Acknowledgement instruction (RFC 9204 §4.4.1) into `buf`.
///
/// Format: `1XXXXXXX` with a 7-bit prefix integer for the stream ID.
fn encode_section_ack(stream_id: u64, buf: &mut Vec<u8>) {
    let mut encoded = varint::encode(usize::try_from(stream_id).unwrap_or(usize::MAX), 7);
    encoded[0] |= DEC_INSTR_SECTION_ACK;
    buf.extend_from_slice(&encoded);
}

/// Encode an Insert Count Increment instruction (RFC 9204 §4.4.3) into `buf`.
///
/// Format: `00XXXXXX` with a 6-bit prefix integer for the increment.
fn encode_insert_count_increment(increment: u64, buf: &mut Vec<u8>) {
    let mut encoded = varint::encode(usize::try_from(increment).unwrap_or(usize::MAX), 6);
    encoded[0] |= DEC_INSTR_INSERT_COUNT_INC; // 0x00 — no-op, but makes the intent explicit
    buf.extend_from_slice(&encoded);
}

async fn write(
    buf: &mut Vec<u8>,
    mut stream: impl AsyncWrite + Unpin + Send,
    mut f: impl FnMut(&mut [u8]) -> Option<usize>,
) -> io::Result<usize> {
    let written = loop {
        if let Some(w) = f(buf) {
            break w;
        }
        if buf.len() >= MAX_BUFFER_SIZE {
            return Err(io::Error::new(ErrorKind::OutOfMemory, "runaway allocation"));
        }
        buf.resize(buf.len() * 2, 0);
    };

    stream.write_all(&buf[..written]).await?;
    stream.flush().await?;
    Ok(written)
}
