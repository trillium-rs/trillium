#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
use std::convert::Infallible;

pub use async_trait::async_trait;
pub use futures_lite::{AsyncRead, AsyncWrite};

#[async_trait]
pub trait Acceptor<Input>: Clone + Send + Sync + 'static
where
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static;
    type Error: std::fmt::Debug + Send + Sync;

    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error>;
}

#[async_trait]
impl<Input> Acceptor<Input> for ()
where
    Input: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = Input;
    type Error = Infallible;
    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        Ok(input)
    }
}
