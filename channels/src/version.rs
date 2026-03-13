use std::{convert::Infallible, str::FromStr};

/// The phoenix channel "protocol" version
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
#[derive(Default)]
pub enum Version {
    /// the implicit first version of the protocol
    #[default]
    V1,

    /// version 2.x of the protocol
    V2,
}

impl FromStr for Version {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.chars().next() {
            Some('2') => Ok(Self::V2),
            _ => Ok(Self::V1),
        }
    }
}

impl From<&str> for Version {
    fn from(s: &str) -> Self {
        s.parse().unwrap()
    }
}
