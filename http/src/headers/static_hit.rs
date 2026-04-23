//! Result of looking up a field line in a static table (HPACK or QPACK).
//!
//! Both protocols have differently-populated static tables but the lookup result has
//! the same three-way shape: full pair match, name-only match, or no match. One enum,
//! one consumer language for the encoder/cost-model arms.

/// A static-table lookup result. The index payload is 1-based for HPACK (RFC 7541)
/// and 0-based for QPACK (RFC 9204); each protocol's lookup function is responsible
/// for using the right convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, fieldwork::Fieldwork)]
#[fieldwork(get)]
pub(in crate::headers) enum StaticHit {
    /// Both name and value match a static table entry.
    Full(#[field = "full"] u8),
    /// Name matches but value doesn't.
    Name(#[field = "name"] u8),
    /// Name not in the static table.
    None,
}
