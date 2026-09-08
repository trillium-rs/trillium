//! HTTP/1.x field-line parsing into [`Headers`].

use super::{HeaderName, HeaderNameInner, HeaderValue, HeaderValues, Headers};
use crate::Error;
use hashbrown::hash_map::EntryRef;
use memchr::memmem::Finder;

impl Headers {
    /// Parse `bytes` as a sequence of CRLF-terminated field lines and append each to this map.
    ///
    /// Returns the number of field lines appended. On the first malformed line, returns `Err`
    /// with every preceding line already appended.
    #[doc(hidden)]
    pub fn extend_parse(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        self.extend_parse_inner(bytes, None)
    }

    /// Like [`extend_parse`](Self::extend_parse), but moves matching entries out of `previous`
    /// rather than building fresh ones.
    ///
    /// A field line is taken from `previous` only when `previous` holds exactly one value for
    /// that name and its bytes equal the parsed value exactly. Nothing else from `previous` is
    /// observable in the result, which matters when a proxy coalesces requests from many clients
    /// onto one keepalive connection: a value only carries over if the new request sent
    /// identical bytes.
    pub(crate) fn extend_parse_reusing(
        &mut self,
        bytes: &[u8],
        previous: &mut Headers,
    ) -> Result<usize, Error> {
        self.extend_parse_inner(bytes, Some(previous))
    }

    fn extend_parse_inner(
        &mut self,
        bytes: &[u8],
        mut previous: Option<&mut Headers>,
    ) -> Result<usize, Error> {
        let mut new_header_count = 0;
        let mut reused = 0;
        let mut last_line = 0;
        for newline in Finder::new(b"\r\n").find_iter(bytes) {
            if newline == last_line {
                continue;
            }

            let line = &bytes[last_line..newline];

            // Validate each field line as it's parsed, appending the valid ones as we go. On the
            // first violation we return `Err` with `self` still holding everything before it, so
            // the request parser can synthesize a response from the partial parse
            // rather than closing blind. A line with no colon — e.g. an obs-fold
            // continuation — has no name and is rejected (obs-fold is forbidden in requests).
            let colon = memchr::memchr(b':', line).ok_or(Error::InvalidHeaderName)?;
            let name = HeaderName::parse(&line[..colon])?;
            if !name.is_valid() {
                return Err(Error::InvalidHeaderName);
            }

            let mut value_start = colon + 1;
            while line
                .get(value_start)
                .is_some_and(|b| matches!(b, b'\t' | b' '))
            {
                value_start += 1;
            }
            let value_bytes = line[value_start..].trim_ascii_end();
            // A field value carries no C0 control except HTAB; obs-text (`0x80..=0xFF`) is allowed.
            if !value_bytes.iter().all(|&b| b >= 0x20 || b == b'\t') {
                return Err(Error::InvalidHeaderValue(name.to_owned()));
            }

            match previous
                .as_deref_mut()
                .and_then(|previous| previous.take_matching(&name, value_bytes))
            {
                Some((name, values)) => {
                    reused += 1;
                    self.append(name, values);
                }
                None => {
                    self.append(name.to_owned(), HeaderValue::parse(value_bytes));
                }
            }
            new_header_count += 1;
            last_line = newline + 2;
        }
        if previous.is_some() {
            log::trace!("h1 headers: reused {reused} of {new_header_count} field lines");
        }
        Ok(new_header_count)
    }

    /// Remove and return the entry for `name` if it holds exactly one value whose bytes equal
    /// `value_bytes`.
    fn take_matching(
        &mut self,
        name: &HeaderName<'_>,
        value_bytes: &[u8],
    ) -> Option<(HeaderName<'static>, HeaderValues)> {
        let matches = |values: &HeaderValues| values.one().is_some_and(|v| v == value_bytes);
        match &name.0 {
            HeaderNameInner::KnownHeader(known) => match self.known.entry_ref(known) {
                EntryRef::Occupied(entry) if matches(entry.get()) => {
                    let (name, values) = entry.remove_entry();
                    Some((name.into(), values))
                }
                _ => None,
            },
            HeaderNameInner::UnknownHeader(unknown) => match self.unknown.entry_ref(&unknown) {
                EntryRef::Occupied(entry) if matches(entry.get()) => {
                    let (name, values) = entry.remove_entry();
                    Some((name.into(), values))
                }
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KnownHeaderName;

    const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";

    fn parsed(bytes: &[u8]) -> Headers {
        let mut headers = Headers::new();
        headers.extend_parse(bytes).unwrap();
        headers
    }

    fn str_ptr(headers: &Headers, name: impl Into<HeaderName<'static>>) -> *const u8 {
        headers.get_str(name).unwrap().as_ptr()
    }

    #[test]
    fn identical_values_move_from_previous() {
        let request = format!("user-agent: {UA}\r\nx-long-custom-header-name-x: {UA}\r\n");
        let mut previous = parsed(request.as_bytes());
        let ua_ptr = str_ptr(&previous, KnownHeaderName::UserAgent);
        let custom_ptr = str_ptr(&previous, "x-long-custom-header-name-x");

        let mut next = Headers::new();
        next.extend_parse_reusing(request.as_bytes(), &mut previous)
            .unwrap();

        assert_eq!(str_ptr(&next, KnownHeaderName::UserAgent), ua_ptr);
        assert_eq!(str_ptr(&next, "x-long-custom-header-name-x"), custom_ptr);
        assert!(
            previous.is_empty(),
            "taken entries leave previous: {previous:?}"
        );
    }

    #[test]
    fn differing_values_are_not_taken() {
        let mut previous = parsed(format!("user-agent: {UA}\r\nhost: a.example\r\n").as_bytes());
        let ua_ptr = str_ptr(&previous, KnownHeaderName::UserAgent);

        let mut next = Headers::new();
        next.extend_parse_reusing(
            format!("user-agent: {UA} Extra\r\nhost: b.example\r\n").as_bytes(),
            &mut previous,
        )
        .unwrap();

        assert_ne!(str_ptr(&next, KnownHeaderName::UserAgent), ua_ptr);
        assert_eq!(
            next.get_str(KnownHeaderName::UserAgent).unwrap(),
            format!("{UA} Extra")
        );
        assert_eq!(next.get_str(KnownHeaderName::Host), Some("b.example"));
        assert_eq!(previous.len(), 2, "untaken entries stay in previous");
    }

    #[test]
    fn multi_valued_previous_entries_are_not_taken() {
        let mut previous = parsed(b"x-a: one\r\nx-a: two\r\n");
        let mut next = Headers::new();
        next.extend_parse_reusing(b"x-a: one\r\n", &mut previous)
            .unwrap();
        assert_eq!(next.get_values("x-a").unwrap().len(), 1);
        assert_eq!(previous.get_values("x-a").unwrap().len(), 2);
    }

    #[test]
    fn repeated_lines_in_current_request_append() {
        let mut previous = parsed(b"x-a: one\r\n");
        let mut next = Headers::new();
        next.extend_parse_reusing(b"x-a: one\r\nx-a: one\r\n", &mut previous)
            .unwrap();
        assert_eq!(next.get_values("x-a").unwrap().len(), 2);
        assert!(previous.is_empty());
    }
}
