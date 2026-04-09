//! QPACK encoder stream processing (RFC 9204 §3.2).
//!
//! The encoder stream is a unidirectional stream sent by the peer carrying instructions
//! that modify the dynamic table: Set Dynamic Table Capacity, Insert With Name Reference,
//! Insert With Literal Name, and Duplicate.

use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{
        decoder_dynamic_table::DecoderDynamicTable,
        entry_name::QpackEntryName,
        instruction::encoder::{EncoderInstruction, parse},
        static_table::static_entry,
    },
};
use futures_lite::io::AsyncRead;
use std::borrow::Cow;

impl DecoderDynamicTable {
    /// Process a QPACK encoder stream, applying each instruction to `table`.
    ///
    /// Reads a continuous stream of encoder instructions (Set Dynamic Table Capacity, Insert
    /// With Name Reference, Insert With Literal Name, Duplicate) and applies them to the
    /// connection's dynamic table. Returns when the stream closes or an error occurs; on
    /// error, marks the table as failed so blocked decode futures are woken with an error.
    ///
    /// # Errors
    ///
    /// Returns an `H3Error` on I/O failure or protocol error.
    pub(crate) async fn run_reader<T: AsyncRead + Unpin + Send>(
        &self,
        stream: &mut T,
    ) -> Result<(), H3Error> {
        let result = self.run_reader_inner(stream).await;
        match &result {
            Err(H3Error::Protocol(code)) => {
                log::debug!("QPACK encoder stream: protocol error: {code}");
                self.fail(*code);
            }

            Err(H3Error::Io(e)) => {
                log::debug!("QPACK encoder stream: I/O error: {e}");
                self.fail(H3ErrorCode::QpackEncoderStreamError);
            }

            Ok(()) => {
                log::trace!("QPACK encoder stream: closed cleanly");
            }
        }

        result
    }

    async fn run_reader_inner<T>(&self, stream: &mut T) -> Result<(), H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        let max_entry_size = self.max_capacity();
        while let Some(instruction) = parse(max_entry_size, stream).await? {
            self.apply(instruction)?;
        }
        log::trace!("QPACK encoder stream: EOF");
        Ok(())
    }

    fn apply(&self, instruction: EncoderInstruction) -> Result<(), H3Error> {
        match instruction {
            EncoderInstruction::SetCapacity(capacity) => {
                log::trace!("QPACK encoder: Set Dynamic Table Capacity {capacity}");
                self.set_capacity(capacity).inspect_err(|e| {
                    log::error!("QPACK encoder: set_capacity({capacity}) failed: {e:?}");
                })
            }

            EncoderInstruction::InsertWithStaticNameRef { name_index, value } => {
                let (static_name, _) = static_entry(name_index).map_err(|e| {
                    log::error!("QPACK encoder: static_entry({name_index}) failed: {e:?}");
                    H3ErrorCode::QpackEncoderStreamError
                })?;
                let name = QpackEntryName::from(*static_name);
                log::trace!(
                    "QPACK encoder: Insert With Name Reference (static) [{name}: {}]",
                    String::from_utf8_lossy(&value)
                );
                self.insert(name, Cow::Owned(value))
            }

            EncoderInstruction::InsertWithDynamicNameRef {
                relative_index,
                value,
            } => {
                let name = self.name_at_relative(relative_index).ok_or_else(|| {
                    log::error!("QPACK encoder: name_at_relative({relative_index}) returned None");
                    H3ErrorCode::QpackEncoderStreamError
                })?;
                log::trace!(
                    "QPACK encoder: Insert With Name Reference (dynamic) [{name}: {}]",
                    String::from_utf8_lossy(&value)
                );
                self.insert(name, Cow::Owned(value))
            }

            EncoderInstruction::InsertWithLiteralName { name, value } => {
                log::trace!(
                    "QPACK encoder: Insert With Literal Name [{name}: {}]",
                    String::from_utf8_lossy(&value)
                );
                self.insert(name, Cow::Owned(value))
            }

            EncoderInstruction::Duplicate { relative_index } => {
                log::trace!("QPACK encoder: Duplicate index {relative_index}");
                self.duplicate(relative_index).inspect_err(|e| {
                    log::error!("QPACK encoder: duplicate({relative_index}) failed: {e:?}");
                })
            }
        }
    }
}
