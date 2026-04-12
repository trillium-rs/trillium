//! Reads the peer's QPACK decoder stream (RFC 9204 §4.4).
//!
//! Runs as a connection-scoped task: parses a continuous stream of Section Acknowledgement,
//! Stream Cancellation, and Insert Count Increment instructions and feeds them into the
//! [`EncoderDynamicTable`]. Returns on clean EOF, shutdown, I/O error, or protocol error;
//! on protocol/I/O error the table is marked failed so the encoder-stream writer exits too.

use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::qpack::EncoderDynamicTable,
};
use futures_lite::io::{AsyncRead, AsyncReadExt};

// Section Acknowledgement: `1xxxxxxx` with a 7-bit prefix integer stream ID.
const DEC_INSTR_SECTION_ACK: u8 = 0x80;
// Stream Cancellation: `01xxxxxx` with a 6-bit prefix integer stream ID.
const DEC_INSTR_STREAM_CANCEL: u8 = 0x40;
// Insert Count Increment: `00xxxxxx` with a 6-bit prefix integer increment.
// (High bits are zero; no constant needed for matching.)

impl EncoderDynamicTable {
    /// Process the peer's decoder-stream instructions and apply them to `table`.
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
    loop {
        let Some(first) = read_first_byte(stream).await? else {
            return Ok(());
        };

        if first & DEC_INSTR_SECTION_ACK != 0 {
            let stream_id = read_varint(first, 7, stream).await?;
            log::trace!(
                "QPACK decoder stream reader: Section Acknowledgement stream_id={stream_id}"
            );
            table.on_section_ack(stream_id as u64)?;
        } else if first & DEC_INSTR_STREAM_CANCEL != 0 {
            let stream_id = read_varint(first, 6, stream).await?;
            log::trace!("QPACK decoder stream reader: Stream Cancellation stream_id={stream_id}");
            table.on_stream_cancel(stream_id as u64);
        } else {
            let increment = read_varint(first, 6, stream).await?;
            log::trace!("QPACK decoder stream reader: Insert Count Increment {increment}");
            table.on_insert_count_increment(increment as u64)?;
        }
    }
}

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
        .map_err(|_| H3ErrorCode::QpackDecoderStreamError)?;
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
    let mut shift = 0_u32;
    loop {
        let byte = read_byte(stream).await?;
        let payload = usize::from(byte & 0x7F);
        let increment = payload
            .checked_shl(shift)
            .ok_or(H3ErrorCode::QpackDecoderStreamError)?;
        value = value
            .checked_add(increment)
            .ok_or(H3ErrorCode::QpackDecoderStreamError)?;
        shift += 7;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeaderName, HeaderValue, headers::qpack::encoder_dynamic_table::SectionRefs};
    use futures_lite::future::block_on;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from(s.as_bytes().to_vec())
    }

    fn hn(s: &str) -> HeaderName<'static> {
        HeaderName::parse(s.as_bytes()).unwrap().into_owned()
    }

    fn make_table_with_two_entries() -> EncoderDynamicTable {
        let table = EncoderDynamicTable::new(4096);
        table.enqueue_set_capacity(4096).unwrap();
        table.enqueue_insert_literal(hn("a"), hv("1")).unwrap();
        table.enqueue_insert_literal(hn("b"), hv("2")).unwrap();
        table
    }

    fn push_section(table: &EncoderDynamicTable, stream_id: u64, ric: u64, min_ref: Option<u64>) {
        table.push_outstanding_section_for_test(
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
        let table = EncoderDynamicTable::new(4096);
        // Section Ack with no outstanding section is a protocol error.
        let mut wire: &[u8] = &[0x84];
        let err = block_on(table.run_reader(&mut wire));
        assert!(err.is_err());
        assert!(table.failed().is_some());
    }

    #[test]
    fn clean_eof_returns_ok() {
        let table = EncoderDynamicTable::new(4096);
        let mut wire: &[u8] = &[];
        block_on(table.run_reader(&mut wire)).unwrap();
        assert!(table.failed().is_none());
    }
}
