//! HTTP priority signals, as defined by [RFC 9218][rfc] (Extensible Prioritization
//! Scheme for HTTP).
//!
//! A client expresses how it would like a server to schedule a response relative to
//! other concurrent responses on the same connection — both how urgent it is and
//! whether it can be delivered incrementally. The signal is advisory: a server is free
//! to use it, adapt it, or ignore it.
//!
//! [rfc]: https://www.rfc-editor.org/rfc/rfc9218

mod parse;

use std::{
    fmt::{self, Display, Formatter},
    str::FromStr,
};

const DEFAULT_URGENCY: u8 = 3;
const MAX_URGENCY: u8 = 7;

/// A response priority: an urgency level and an incremental flag.
///
/// Urgency runs from 0 (most urgent) to 7 (least urgent), defaulting to 3. The
/// incremental flag indicates whether the response can be usefully processed as it
/// arrives (e.g. a progressively-rendered image) rather than only once complete;
/// servers may interleave incremental responses of equal urgency.
///
/// Parse one from a `priority` header value with [`Priority::parse`] (or [`FromStr`]), or start
/// from [`Priority::default`]; render it back to header form with [`Display`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Priority {
    urgency: u8,
    incremental: bool,
}

impl Default for Priority {
    fn default() -> Self {
        Self {
            urgency: DEFAULT_URGENCY,
            incremental: false,
        }
    }
}

impl Priority {
    /// Construct a non-incremental priority with the given urgency.
    ///
    /// Urgency is clamped to the valid range 0..=7.
    #[must_use]
    #[cfg(test)] // builder API deferred with the handler-facing priority surface
    pub fn new(urgency: u8) -> Self {
        Self {
            urgency: urgency.min(MAX_URGENCY),
            incremental: false,
        }
    }

    /// The urgency level, 0 (most urgent) through 7 (least urgent).
    #[must_use]
    pub const fn urgency(self) -> u8 {
        self.urgency
    }

    /// Whether the response may be delivered incrementally.
    #[must_use]
    pub const fn is_incremental(self) -> bool {
        self.incremental
    }

    /// Set the urgency, clamped to the valid range 0..=7.
    #[must_use]
    #[cfg(test)] // builder API deferred with the handler-facing priority surface
    pub fn with_urgency(mut self, urgency: u8) -> Self {
        self.urgency = urgency.min(MAX_URGENCY);
        self
    }

    /// Set the incremental flag.
    #[must_use]
    #[cfg(test)] // builder API deferred with the handler-facing priority surface
    pub const fn with_incremental(mut self, incremental: bool) -> Self {
        self.incremental = incremental;
        self
    }

    /// Parse a `priority` header value. Unrecognized, missing, or malformed fields fall back to
    /// their defaults rather than erroring, as the scheme mandates, so this is infallible.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        parse::parse(s)
    }
}

impl FromStr for Priority {
    type Err = std::convert::Infallible;

    /// Infallible — see [`Priority::parse`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

impl Display for Priority {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "u={}", self.urgency)?;
        if self.incremental {
            f.write_str(", i")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::Priority;

    #[test]
    fn defaults() {
        let p = Priority::default();
        assert_eq!(p.urgency(), 3);
        assert!(!p.is_incremental());
    }

    #[test]
    fn constructors_clamp() {
        assert_eq!(Priority::new(5).urgency(), 5);
        assert_eq!(Priority::new(200).urgency(), 7);
        assert_eq!(Priority::default().with_urgency(99).urgency(), 7);
        assert!(Priority::new(0).with_incremental(true).is_incremental());
    }

    #[test]
    fn display_roundtrip() {
        assert_eq!(Priority::new(3).to_string(), "u=3");
        assert_eq!(
            Priority::new(0).with_incremental(true).to_string(),
            "u=0, i"
        );
        for urgency in 0..=7 {
            for incremental in [false, true] {
                let p = Priority::new(urgency).with_incremental(incremental);
                assert_eq!(p.to_string().parse::<Priority>().unwrap(), p);
            }
        }
    }
}
