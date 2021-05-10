use crate::{chunked_encoder::ChunkedEncoder, http_types::Body};
use futures_lite::io::AsyncRead;
use pin_project::pin_project;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

#[pin_project(project=BodyEncoderProjection)]
#[derive(Debug)]
/// A http encoder for [`http_types::Body`]. You probably don't want
/// to interact with this directly.
pub enum BodyEncoder {
    /// a chunked body
    Chunked(#[pin] ChunkedEncoder<Body>),

    /// a fixed-length body
    Fixed(#[pin] Body),
}

impl BodyEncoder {
    /// builds a body encoder for the provided [`http_types::Body`]
    pub fn new(body: Body) -> Self {
        match body.len() {
            Some(_) => Self::Fixed(body),
            None => Self::Chunked(ChunkedEncoder::new(body)),
        }
    }
}

impl AsyncRead for BodyEncoder {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.project() {
            BodyEncoderProjection::Chunked(encoder) => encoder.poll_read(cx, buf),
            BodyEncoderProjection::Fixed(body) => body.poll_read(cx, buf),
        }
    }
}
