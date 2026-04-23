//! HPACK header compression (RFC 7541).
//!
//! HPACK is HTTP/2's header compression format. Compared to QPACK (RFC 9204) it is simpler:
//! inserts into the dynamic table happen inline with the containing header block rather
//! than on a separate encoder stream, so there is no out-of-order delivery, no blocking,
//! no section acknowledgements, and a single [`DynamicTable`] serves both the decoder and
//! the encoder within a connection.
//!
//! This module reuses the shared primitives out of `headers/`:
//! - [`crate::headers::huffman`] — RFC 7541 Appendix B code (identical for HPACK/QPACK).
//! - [`crate::headers::integer_prefix`] — RFC 7541 §5.1 prefix integer codec.
//! - [`crate::headers::entry_name::EntryName`] / `PseudoHeaderName` — shared name vocabulary.
//! - [`crate::headers::field_section::FieldSection`] — decoded output shape.
//! - [`crate::headers::compression_error::CompressionError`] — shared codec error.
//!
//! The module-wide `dead_code` allow is transient — the static table's lookup is consumed
//! by the HPACK decoder, which lands next. Remove once that happens.
#![allow(dead_code)]

pub(crate) mod decoder;
pub(crate) mod dynamic_table;
pub(crate) mod static_table;
