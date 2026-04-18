//! Shared QIF (QPACK Interop Format) helpers for test harnesses.
//!
//! QIF files are the canonical human-readable representation of a sequence of HTTP header
//! blocks used by the [qpackers/qifs](https://github.com/qpackers/qifs) offline-interop corpus.
//! Each file is a series of groups separated by blank lines; each group is one or more
//! `name\tvalue` lines representing a single request or response. Pseudo-headers are prefixed
//! with `:` (e.g. `:method`, `:status`). Comments (`#` prefix) are ignored.
//!
//! Both the decoder corpus test (`corpus_tests.rs`) and the encoder corpus test
//! (`encoder_corpus_tests.rs`) need to parse QIF files and compare against them in
//! `(name, value)` form, so the helpers live here.

use super::{FieldLineValue, FieldSection, PseudoHeaders, entry_name::QpackEntryName};
use crate::Status;

/// A parsed QIF group — a flat list of `(name, value)` pairs for one request or response,
/// including pseudo-headers (with their `:` prefix preserved).
pub(super) type QifGroup = Vec<(String, String)>;

/// Parse QIF content into an ordered list of header groups.
///
/// Each group is a `Vec<(name, value)>` of all headers (including pseudo-headers) for one
/// stream, in file order. Blank lines separate groups; `#`-prefixed lines are comments. The
/// name/value separator within a line is a tab character.
pub(super) fn parse(content: &str) -> Vec<QifGroup> {
    let mut groups: Vec<QifGroup> = Vec::new();
    let mut current: QifGroup = Vec::new();

    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some((name, value)) = line.split_once('\t') {
            current.push((name.to_owned(), value.to_owned()));
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// Convert a decoded `FieldSection` into a comparable `QifGroup`.
///
/// * Header names are lowercased (HTTP/3 §4.2 requires lowercase on the wire, but the decoder
///   stores static-table known headers in their canonical HTTP/1.1 casing).
/// * `:status` is emitted as the numeric code only (`"200"`), matching QIF convention.
pub(super) fn field_section_to_pairs(fs: FieldSection<'static>) -> QifGroup {
    let (pseudos, headers) = fs.into_parts();
    let mut pairs = Vec::new();
    pseudo_pairs(&pseudos, &mut pairs);
    for (name, values) in headers.iter() {
        for value in values {
            pairs.push((name.to_string().to_lowercase(), value.to_string()));
        }
    }
    pairs
}

fn pseudo_pairs(p: &PseudoHeaders<'_>, out: &mut QifGroup) {
    if let Some(m) = p.method() {
        out.push((":method".into(), m.to_string()));
    }
    if let Some(s) = p.status() {
        out.push((":status".into(), status_code_str(s)));
    }
    if let Some(v) = p.path() {
        out.push((":path".into(), v.to_string()));
    }
    if let Some(v) = p.scheme() {
        out.push((":scheme".into(), v.to_string()));
    }
    if let Some(v) = p.authority() {
        out.push((":authority".into(), v.to_string()));
    }
    if let Some(v) = p.protocol() {
        out.push((":protocol".into(), v.to_string()));
    }
}

/// Format a `Status` as its numeric code string (`"200"`, `"404"`, …) to match QIF, which
/// carries the code alone without the reason phrase that `Status`'s `Display` includes.
fn status_code_str(s: Status) -> String {
    s.to_string()
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .to_owned()
}

/// Convert a parsed QIF group into an ordered list of `(QpackEntryName, FieldLineValue)`
/// pairs suitable for [`super::EncoderDynamicTable::encode_field_lines`].
///
/// File order is preserved verbatim — both for stable wire output (the encoder is
/// order-sensitive: it inserts dynamic-table entries in the order it encounters them) and
/// for HTTP/3 semantic fidelity (header order on the wire is observable).
///
/// Used by the encoder corpus test only. Returns an error string on a name that doesn't
/// parse as a known header, pseudo-header, or valid unknown header so the caller can
/// attribute the failure to a specific qif file and group.
pub(super) fn build_field_lines(
    group: &QifGroup,
) -> Result<Vec<(QpackEntryName<'static>, FieldLineValue<'static>)>, String> {
    group
        .iter()
        .map(|(name, value)| {
            let entry_name = QpackEntryName::try_from(name.as_bytes().to_vec())
                .map_err(|e| format!("parsing name {name:?}: {e:?}"))?;
            Ok((
                entry_name,
                FieldLineValue::Owned(value.clone().into_bytes()),
            ))
        })
        .collect()
}
