use super::{HeaderName, HeaderNameInner::UnknownHeader};
use hashbrown::{Equivalent, HashSet};
use smartcow::SmartCow;
use std::{
    cmp::Ordering,
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    ops::Deref,
    sync::{OnceLock, RwLock},
};

#[derive(Clone)]
pub(crate) struct UnknownHeaderName<'a>(SmartCow<'a>);

impl UnknownHeaderName<'_> {
    pub(crate) fn is_valid_lower(&self) -> bool {
        // Lowercase tchar — the uppercase-letter branch is dropped because HTTP/2 and
        // HTTP/3 require field names to be lowercase on the wire. Otherwise matches
        // `is_tchar`.
        !self.is_empty()
            && self.chars().all(|c| {
                matches!(c,
                    'a'..='z'
                    | '0'..='9'
                    | '!'
                    | '#'
                    | '$'
                    | '%'
                    | '&'
                    | '\''
                    | '*'
                    | '+'
                    | '-'
                    | '.'
                    | '^'
                    | '_'
                    | '`'
                    | '|'
                    | '~',
                )
            })
    }

    pub(crate) fn into_lower(self) -> Self {
        match self.0 {
            SmartCow::Borrowed(borrowed) => {
                if let Some(first_upper) = borrowed.chars().position(|c| c.is_ascii_uppercase()) {
                    Self(SmartCow::Owned(
                        borrowed[..first_upper]
                            .chars()
                            .chain(
                                borrowed[first_upper..]
                                    .chars()
                                    .map(|c| c.to_ascii_lowercase()),
                            )
                            .collect(),
                    ))
                } else {
                    Self(SmartCow::Borrowed(borrowed))
                }
            }
            SmartCow::Owned(mut smart_string) => {
                smart_string.make_ascii_lowercase();
                Self(SmartCow::Owned(smart_string))
            }
        }
    }
}

impl PartialOrd for UnknownHeaderName<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for UnknownHeaderName<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&*other.0)
    }
}

impl PartialEq for UnknownHeaderName<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl Eq for UnknownHeaderName<'_> {}

impl Hash for UnknownHeaderName<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for c in self.0.as_bytes() {
            c.to_ascii_lowercase().hash(state);
        }
    }
}

impl Debug for UnknownHeaderName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl Display for UnknownHeaderName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<'a> From<UnknownHeaderName<'a>> for HeaderName<'a> {
    fn from(value: UnknownHeaderName<'a>) -> Self {
        HeaderName(UnknownHeader(value))
    }
}

impl<'a> From<&'a UnknownHeaderName<'_>> for HeaderName<'a> {
    fn from(value: &'a UnknownHeaderName<'_>) -> Self {
        HeaderName(UnknownHeader(value.reborrow()))
    }
}

fn is_tchar(c: char) -> bool {
    matches!(
        c,
        'a'..='z'
        | 'A'..='Z'
        | '0'..='9'
        | '!'
        | '#'
        | '$'
        | '%'
        | '&'
        | '\''
        | '*'
        | '+'
        | '-'
        | '.'
        | '^'
        | '_'
        | '`'
        | '|'
        | '~'
    )
}

impl UnknownHeaderName<'_> {
    pub(crate) fn is_valid(&self) -> bool {
        !self.is_empty() && self.0.chars().all(is_tchar)
    }

    pub(crate) fn into_owned(self) -> UnknownHeaderName<'static> {
        UnknownHeaderName(self.0.into_owned())
    }
}

impl<'a> UnknownHeaderName<'a> {
    pub(crate) fn reborrow<'b: 'a>(&'b self) -> UnknownHeaderName<'b> {
        Self(self.0.borrow())
    }
}

impl From<String> for UnknownHeaderName<'static> {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl<'a> From<&'a str> for UnknownHeaderName<'a> {
    fn from(value: &'a str) -> Self {
        Self(value.into())
    }
}

impl<'a> From<SmartCow<'a>> for UnknownHeaderName<'a> {
    fn from(value: SmartCow<'a>) -> Self {
        Self(value)
    }
}

impl<'a> From<UnknownHeaderName<'a>> for SmartCow<'a> {
    fn from(value: UnknownHeaderName<'a>) -> Self {
        value.0
    }
}

impl Deref for UnknownHeaderName<'_> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Equivalent<UnknownHeaderName<'_>> for &UnknownHeaderName<'_> {
    fn equivalent(&self, key: &UnknownHeaderName<'_>) -> bool {
        key.eq_ignore_ascii_case(self)
    }
}

/// Process-global table of canonical lowercased `&'static str` for literal
/// header names that contained uppercase characters in source. Pure-lowercase
/// literals bypass this table entirely (no need to intern — they're already
/// `&'static`).
///
/// `RwLock` because once the application has exercised each uppercase literal
/// once, the table is steady-state read-only. The hasher is case-insensitive so
/// we can probe with the original uppercase input without first allocating its
/// lowercased form.
///
/// Bounded above by distinct uppercase-containing lowercased literal names in
/// the binary.
static LOWER_INTERN: OnceLock<RwLock<HashSet<InternKey>>> = OnceLock::new();

/// Wrapper around `&'static str` whose `Hash`/`Eq` are case-insensitive on
/// ASCII. Lets the interner store the canonical lowercased form once and probe
/// with the original casing without rebuilding the lowercased string just to
/// look it up.
#[derive(Copy, Clone, Eq)]
struct InternKey(&'static str);

impl PartialEq for InternKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(other.0)
    }
}

impl Hash for InternKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for b in self.0.bytes() {
            state.write_u8(b.to_ascii_lowercase());
        }
    }
}

fn intern_table() -> &'static RwLock<HashSet<InternKey>> {
    LOWER_INTERN.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Return a canonical lowercased `&'static str` for `s`.
///
/// - If `s` is already all-lowercase: returns `s` directly. No lock, no alloc.
/// - Otherwise: probes the intern table with a case-insensitive hash. On hit, returns the stored
///   canonical pointer. On miss, allocates the lowercased form, leaks it to obtain a `&'static
///   str`, and inserts.
///
/// The leak is bounded by the number of distinct uppercase-containing lowercased
/// literals in the binary — typically zero or single digits for well-behaved code.
fn intern_lowercase(s: &'static str) -> &'static str {
    if !s.bytes().any(|b| b.is_ascii_uppercase()) {
        return s;
    }
    let probe = InternKey(s);
    let table = intern_table();
    {
        let read = table.read().expect("intern table poisoned");
        if let Some(hit) = read.get(&probe) {
            return hit.0;
        }
    }
    // Allocate-and-leak inside the write lock so two threads racing for the same
    // uppercase literal don't both leak: the second arrival finds the first's
    // insert via the post-acquire `get` and bails before allocating.
    let mut write = table.write().expect("intern table poisoned");
    if let Some(hit) = write.get(&probe) {
        return hit.0;
    }
    let lowered: String = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    let leaked: &'static str = Box::leak(lowered.into_boxed_str());
    write.insert(InternKey(leaked));
    leaked
}

impl UnknownHeaderName<'static> {
    /// Recover the underlying `&'static str` if this name is backed by a borrowed
    /// reference into static memory (a literal or an interned lowercased literal).
    /// Returns `None` for runtime-allocated names (`SmartCow::Owned`).
    pub(crate) fn as_static_str(&self) -> Option<&'static str> {
        match self.0 {
            SmartCow::Borrowed(s) => Some(s),
            SmartCow::Owned(_) => None,
        }
    }

    /// Like [`Self::into_lower`], but for the uppercase-borrowed-static case it
    /// interns the lowercased form via [`intern_lowercase`] instead of allocating
    /// an Owned copy. The result is therefore *always* `SmartCow::Borrowed` (and
    /// hence `&'static str`-recoverable via [`as_static_str`]) when the input was
    /// `SmartCow::Borrowed`. `Owned` inputs fall back to the regular
    /// [`Self::into_lower`] path and are not interned.
    ///
    /// [`as_static_str`]: Self::as_static_str
    pub(crate) fn into_lower_static(self) -> Self {
        match self.0 {
            SmartCow::Borrowed(s) => {
                if s.bytes().any(|b| b.is_ascii_uppercase()) {
                    Self(SmartCow::Borrowed(intern_lowercase(s)))
                } else {
                    Self(SmartCow::Borrowed(s))
                }
            }
            SmartCow::Owned(_) => self.into_lower(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_interned(s: &'static str) -> &'static str {
        intern_lowercase(s)
    }

    #[test]
    fn intern_idempotent() {
        let a = ensure_interned("X-Idempotent-Header");
        let b = ensure_interned("X-Idempotent-Header");
        assert_eq!(a, "x-idempotent-header");
        assert!(
            std::ptr::eq(a, b),
            "intern must return identical &'static str on repeat uppercase input",
        );
    }

    #[test]
    fn intern_lowercase_input_is_passthrough() {
        // Pure-lowercase input bypasses the intern table — caller's pointer is
        // returned directly.
        let original: &'static str = "x-already-lowercase";
        let got = ensure_interned(original);
        assert!(
            std::ptr::eq(got, original),
            "pure-lowercase literal should bypass interning entirely",
        );
    }

    #[test]
    fn intern_uppercase_then_lowercase_content_equal() {
        let upper = ensure_interned("X-Cross-Casing-Header");
        let lower = ensure_interned("x-cross-casing-header");
        assert_eq!(upper, lower);
        // Pointer identity may or may not hold depending on which call ran first.
    }

    #[test]
    fn intern_case_insensitive_hash_collapses_uppercase_variants() {
        // Two different uppercase castings of the same lowercased content must
        // intern to the same pointer.
        let a = ensure_interned("X-Mixed-Casing");
        let b = ensure_interned("x-MIXED-casing");
        assert!(
            std::ptr::eq(a, b),
            "case-insensitive hash must collapse uppercase variants",
        );
    }

    #[test]
    fn into_lower_static_borrowed_uppercase() {
        let n = UnknownHeaderName(SmartCow::Borrowed("X-Static-Upper")).into_lower_static();
        assert_eq!(n.as_static_str(), Some("x-static-upper"));
    }

    #[test]
    fn into_lower_static_borrowed_lowercase_passthrough() {
        let original: &'static str = "x-static-lower";
        let n = UnknownHeaderName(SmartCow::Borrowed(original)).into_lower_static();
        let got = n.as_static_str().unwrap();
        assert!(
            std::ptr::eq(got, original),
            "already-lowercase literal should pass through without interning",
        );
    }

    #[test]
    fn into_lower_static_owned_stays_owned() {
        let owned = UnknownHeaderName::from(String::from("X-Owned-Upper"));
        let lowered = owned.into_lower_static();
        assert_eq!(lowered.as_static_str(), None);
        assert_eq!(&*lowered, "x-owned-upper");
    }
}
