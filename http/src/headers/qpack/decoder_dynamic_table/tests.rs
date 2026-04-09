use super::DecoderDynamicTable;
use crate::headers::qpack::{entry_name::QpackEntryName, static_table::PseudoHeaderName};
use std::borrow::Cow;
use trillium_testing::{harness, test};

#[test(harness)]
async fn insert_and_get_pseudo_header() {
    let table = DecoderDynamicTable::new(4096, usize::MAX);
    table.set_capacity(200).unwrap();
    table
        .insert(
            QpackEntryName::Pseudo(PseudoHeaderName::Method),
            Cow::Owned(b"GET".to_vec()),
        )
        .unwrap();
    let (name, value) = table.get(0, 1).await.unwrap();
    assert_eq!(name, QpackEntryName::Pseudo(PseudoHeaderName::Method));
    assert_eq!(value.as_ref(), b"GET");
}
