use super::Conn;
use std::{
    error::Error,
    fmt::{self, Debug, Display, Formatter},
    ops::{Deref, DerefMut},
};
/// An unexpected http status code was received. Transform this back
/// into the conn with [`From::from`]/[`Into::into`].
///
/// Currently only returned by [`Conn::success`]
#[derive(Debug)]
pub struct UnexpectedStatusError(Box<Conn>);
impl From<Conn> for UnexpectedStatusError {
    fn from(value: Conn) -> Self {
        Self(Box::new(value))
    }
}

impl From<UnexpectedStatusError> for Conn {
    fn from(value: UnexpectedStatusError) -> Self {
        *value.0
    }
}

impl Deref for UnexpectedStatusError {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for UnexpectedStatusError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Error for UnexpectedStatusError {}
impl Display for UnexpectedStatusError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.status() {
            Some(status) => f.write_fmt(format_args!(
                "expected a success (2xx) status code, but got {status}"
            )),
            None => f.write_str("expected a status code to be set, but none was"),
        }
    }
}
