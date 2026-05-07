//! QPACK encoder stream processing (RFC 9204 §3.2).
//!
//! The encoder stream is a unidirectional stream sent by the peer carrying instructions
//! that modify the dynamic table: Set Dynamic Table Capacity, Insert With Name Reference,
//! Insert With Literal Name, and Duplicate.

use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::{
        entry_name::EntryName,
        qpack::{
            decoder_dynamic_table::DecoderDynamicTable,
            instruction::encoder::{EncoderInstruction, parse},
            static_table::static_entry,
        },
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
        // RFC 9204 §4.2: closure of the encoder stream is a connection error of type
        // H3_CLOSED_CRITICAL_STREAM. process_instructions returns Ok on clean EOF; here we
        // promote that to an error and fail the table. (Graceful server-side shutdown drops
        // this future via swansong before reaching that arm; reaching it means the peer
        // FIN'd their encoder stream while our connection was still alive.)
        let result = match self.process_instructions(stream).await {
            Ok(()) => {
                log::debug!("QPACK encoder stream: peer closed (FIN) — H3_CLOSED_CRITICAL_STREAM");
                Err(H3ErrorCode::ClosedCriticalStream.into())
            }
            Err(e) => Err(e),
        };

        match &result {
            Err(H3Error::Protocol(code)) => {
                log::debug!("QPACK encoder stream: protocol error: {code}");
                self.fail(*code);
            }

            Err(H3Error::Io(e)) => {
                log::debug!("QPACK encoder stream: I/O error: {e}");
                self.fail(H3ErrorCode::QpackEncoderStreamError);
            }

            // unreachable given the EOF promotion above; defensively a no-op.
            Ok(()) => {}
        }

        result
    }

    /// Loop-body of [`run_reader`] separated for tests and corpus replay: parse and apply
    /// peer instructions until clean EOF or error, but **do not** convert EOF into
    /// `H3_CLOSED_CRITICAL_STREAM` and **do not** mark the table failed. Production wiring
    /// goes through [`run_reader`], which does both per RFC 9204 §4.2.
    pub(crate) async fn process_instructions<T>(&self, stream: &mut T) -> Result<(), H3Error>
    where
        T: AsyncRead + Unpin + Send,
    {
        let max_entry_size = self.max_capacity();
        while let Some(instruction) = parse(max_entry_size, stream).await? {
            self.apply(instruction)?;
        }
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
                let name = EntryName::from(*static_name);
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
