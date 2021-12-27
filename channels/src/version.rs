use std::{convert::Infallible, str::FromStr};

/**
The phoenix channel "protocol" version
*/
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Version {
    /// the implicit first version of the protocol
    V1,

    /// version 2.x of the protocol
    V2,
}

impl Default for Version {
    fn default() -> Self {
        Self::V1
    }
}

impl FromStr for Version {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.chars().nth(0) {
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
