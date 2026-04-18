//! Reads the peer's QPACK decoder stream (RFC 9204 §4.4).
//!
//! Runs as a connection-scoped task: parses a continuous stream of Section Acknowledgement,
//! Stream Cancellation, and Insert Count Increment instructions via
//! [`instruction::decoder::parse`] and applies them to the [`EncoderDynamicTable`]. Returns
//! on clean EOF, shutdown, I/O error, or protocol error; on protocol/I/O error the table is
//! marked failed so the encoder-stream writer exits too.

use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{
        EncoderDynamicTable,
        instruction::decoder::{DecoderInstruction, parse},
    },
};
use futures_lite::io::AsyncRead;

impl EncoderDynamicTable {
    /// Process the peer's decoder-stream instructions and apply them to `self`.
    ///
    /// Reads until EOF (clean stream close) or a protocol / I/O error. On any error, marks the
    /// table as failed so that the encoder-stream writer task also exits.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` on I/O or protocol failure.
    pub(crate) async fn run_reader<T>(&self, stream: &mut T) -> Result<(), H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        log::trace!("QPACK decoder stream reader: started");
        let result = process(stream, self).await;
        match &result {
            Ok(()) => log::trace!("QPACK decoder stream reader: clean EOF"),
            Err(H3Error::Protocol(code)) => {
                log::debug!("QPACK decoder stream reader: protocol error: {code}");
                self.fail(*code);
            }
            Err(H3Error::Io(e)) => {
                log::debug!("QPACK decoder stream reader: I/O error: {e}");
                self.fail(H3ErrorCode::QpackDecoderStreamError);
            }
        }
        result
    }
}

async fn process<T>(stream: &mut T, table: &EncoderDynamicTable) -> Result<(), H3Error>
where
    T: AsyncRead + Unpin + Send,
{
    while let Some(instruction) = parse(stream).await? {
        apply(table, instruction)?;
    }
    Ok(())
}

fn apply(table: &EncoderDynamicTable, instruction: DecoderInstruction) -> Result<(), H3Error> {
    match instruction {
        DecoderInstruction::SectionAcknowledgement { stream_id } => {
            log::trace!(
                "QPACK decoder stream reader: Section Acknowledgement stream_id={stream_id}"
            );
            table.on_section_ack(stream_id)
        }
        DecoderInstruction::StreamCancellation { stream_id } => {
            log::trace!("QPACK decoder stream reader: Stream Cancellation stream_id={stream_id}");
            table.on_stream_cancel(stream_id);
            Ok(())
        }
        DecoderInstruction::InsertCountIncrement { increment } => {
            log::trace!("QPACK decoder stream reader: Insert Count Increment {increment}");
            table.on_insert_count_increment(increment)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        h3::H3Settings,
        headers::qpack::{
            FieldLineValue, encoder_dynamic_table::SectionRefs, entry_name::QpackEntryName,
        },
    };
    use futures_lite::future::block_on;

    fn qen(s: &str) -> QpackEntryName<'static> {
        QpackEntryName::try_from(s.as_bytes().to_vec()).unwrap()
    }

    fn fv(s: &'static str) -> FieldLineValue<'static> {
        FieldLineValue::Static(s.as_bytes())
    }

    fn make_table_with_two_entries() -> EncoderDynamicTable {
        let table = EncoderDynamicTable::default();
        table.initialize_from_peer_settings(
            4096,
            H3Settings::default().with_qpack_max_table_capacity(4096),
        );
        table.insert(qen("a"), fv("1")).unwrap();
        table.insert(qen("b"), fv("2")).unwrap();
        table
    }

    fn push_section(table: &EncoderDynamicTable, stream_id: u64, ric: u64, min_ref: Option<u64>) {
        table.register_outstanding_section(
            stream_id,
            SectionRefs {
                required_insert_count: ric,
                min_ref_abs_idx: min_ref,
            },
        );
    }

    #[test]
    fn parses_section_ack() {
        let table = make_table_with_two_entries();
        push_section(&table, 4, 2, Some(0));
        // Section Ack for stream ID 4: 0x80 | 4 = 0x84
        let mut wire: &[u8] = &[0x84];
        block_on(table.run_reader(&mut wire)).unwrap();
        assert_eq!(table.known_received_count(), 2);
    }

    #[test]
    fn parses_insert_count_increment() {
        let table = make_table_with_two_entries();
        // ICI increment=1: 0x00 | 1 = 0x01
        let mut wire: &[u8] = &[0x01];
        block_on(table.run_reader(&mut wire)).unwrap();
        assert_eq!(table.known_received_count(), 1);
    }

    #[test]
    fn parses_stream_cancellation() {
        let table = make_table_with_two_entries();
        push_section(&table, 4, 2, Some(0));
        // Stream Cancel stream_id=4: 0x40 | 4 = 0x44
        let mut wire: &[u8] = &[0x44];
        block_on(table.run_reader(&mut wire)).unwrap();
        assert_eq!(table.known_received_count(), 0);
    }

    #[test]
    fn parses_multiple_instructions() {
        let table = make_table_with_two_entries();
        push_section(&table, 4, 1, Some(0));
        // Section Ack stream 4, then ICI +1: expects total known_received = 2.
        let mut wire: &[u8] = &[0x84, 0x01];
        block_on(table.run_reader(&mut wire)).unwrap();
        assert_eq!(table.known_received_count(), 2);
    }

    #[test]
    fn multi_byte_varint() {
        let table = make_table_with_two_entries();
        // Push enough sections that we can ack a large stream id.
        push_section(&table, 200, 2, Some(0));
        // Section Ack for stream 200 needs a multi-byte varint.
        // 7-bit prefix: first byte = 0x80 | 0x7F = 0xFF, then 200 - 127 = 73 = 0x49.
        let mut wire: &[u8] = &[0xFF, 0x49];
        block_on(table.run_reader(&mut wire)).unwrap();
        assert_eq!(table.known_received_count(), 2);
    }

    #[test]
    fn protocol_error_marks_table_failed() {
        let table = EncoderDynamicTable::default();
        // Section Ack with no outstanding section is a protocol error.
        let mut wire: &[u8] = &[0x84];
        let err = block_on(table.run_reader(&mut wire));
        assert!(err.is_err());
        assert!(table.failed().is_some());
    }

    #[test]
    fn clean_eof_returns_ok() {
        let table = EncoderDynamicTable::default();
        let mut wire: &[u8] = &[];
        block_on(table.run_reader(&mut wire)).unwrap();
        assert!(table.failed().is_none());
    }
}
