//! A reference-counted header value shared between a compression dynamic table and the
//! [`Headers`](crate::Headers) it is emitted into.

use super::header_value::HeaderValueInner;
use crate::{HeaderValue, compact_cow::CompactCow, headers::field_section::FieldLineValue};
use std::{
    fmt::{self, Debug, Formatter},
    sync::Arc,
};

/// UTF-8 validity is decided once, when the value is first shared, so every clone can be
/// dropped straight into a [`HeaderValue`] without rescanning the bytes.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum SharedValue {
    Utf8(Arc<str>),
    Bytes(Arc<[u8]>),
}

impl SharedValue {
    pub(in crate::headers) fn as_bytes(&self) -> &[u8] {
        match self {
            SharedValue::Utf8(s) => s.as_bytes(),
            SharedValue::Bytes(b) => b,
        }
    }
}

impl Debug for SharedValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SharedValue::Utf8(s) => Debug::fmt(s, f),
            SharedValue::Bytes(b) => Debug::fmt(&String::from_utf8_lossy(b), f),
        }
    }
}

impl From<&[u8]> for SharedValue {
    fn from(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(s) => SharedValue::Utf8(Arc::from(s)),
            Err(_) => SharedValue::Bytes(Arc::from(bytes)),
        }
    }
}

impl From<Vec<u8>> for SharedValue {
    fn from(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(s) => SharedValue::Utf8(Arc::from(s)),
            Err(e) => SharedValue::Bytes(Arc::from(e.into_bytes())),
        }
    }
}

impl From<FieldLineValue<'_>> for SharedValue {
    fn from(value: FieldLineValue<'_>) -> Self {
        match value {
            FieldLineValue::Static(b) | FieldLineValue::Borrowed(b) => Self::from(b),
            FieldLineValue::Owned(v) => Self::from(v),
            FieldLineValue::Shared(shared) => shared,
        }
    }
}

impl From<SharedValue> for HeaderValue {
    fn from(value: SharedValue) -> Self {
        HeaderValue::from_inner(match value {
            SharedValue::Utf8(s) => HeaderValueInner::Utf8(CompactCow::Shared(s)),
            SharedValue::Bytes(b) => HeaderValueInner::Bytes(b),
        })
    }
}
