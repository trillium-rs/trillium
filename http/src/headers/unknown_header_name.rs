use super::{HeaderName, HeaderNameInner::UnknownHeader};
use hashbrown::Equivalent;
use smartcow::SmartCow;
use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    ops::Deref,
};

#[derive(Clone)]
pub(super) struct UnknownHeaderName<'a>(SmartCow<'a>);

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
        // token per https://www.rfc-editor.org/rfc/rfc9110#section-5.1
        // tchar per https://www.rfc-editor.org/rfc/rfc9110#section-5.6.2
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
