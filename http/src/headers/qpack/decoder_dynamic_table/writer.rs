use crate::{
    h3::{H3Error, MAX_BUFFER_SIZE, UniStreamType, quic_varint},
    headers::qpack::{
        DecoderDynamicTable,
        instruction::decoder::{encode_insert_count_increment, encode_section_ack},
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
            let shutdown = swansong.interrupt(listener).await;
            if shutdown.is_none() {
                log::trace!("QPACK decoder: shutdown");
                break Ok(());
            }
        }
    }
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
