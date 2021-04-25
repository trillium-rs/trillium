use futures_lite::io::{self, AsyncRead, AsyncWrite};
use http_types::headers::{Headers, CONTENT_LENGTH};
use http_types::{Extensions, Method, Version};

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use crate::Stopper;
use crate::{Conn, ReceivedBodyState};

pub struct Synthetic(Option<Vec<u8>>, usize);
impl AsyncWrite for Synthetic {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(0))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Synthetic {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match &self.0 {
            Some(bytes) => {
                let bytes_left = bytes.len() - self.1;
                let bytes_to_read = bytes_left.min(buf.len());
                buf.copy_from_slice(&bytes[self.1..self.1 + bytes_to_read]);
                Poll::Ready(Ok(bytes_to_read))
            }
            None => Poll::Ready(Ok(0)),
        }
    }
}

impl Conn<Synthetic> {
    pub fn new_synthetic(method: Method, path: String, body: Option<Vec<u8>>) -> Self {
        let mut request_headers = Headers::new();
        request_headers.insert(
            CONTENT_LENGTH,
            body.as_ref()
                .map(|b| b.len())
                .unwrap_or_default()
                .to_string(),
        );

        Self {
            rw: Synthetic(body, 0),
            request_headers,
            response_headers: Headers::new(),
            path,
            method,
            status: None,
            version: Version::Http1_1,
            state: Extensions::new(),
            response_body: None,
            buffer: None,
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            stopper: Stopper::new(),
        }
    }

    pub fn request_headers_mut(&mut self) -> &mut Headers {
        &mut self.request_headers
    }
}
