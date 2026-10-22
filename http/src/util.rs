use crate::{Error, HeaderValues, Headers, KnownHeaderName, Result};
use encoding_rs::Encoding;
use futures_lite::{AsyncRead, AsyncReadExt};
use memchr::memmem::Finder;
use mime::Mime;
use std::{io, str::FromStr, sync::LazyLock};

// A `Finder` plans its search strategy on construction, which is wasted work to repeat per parse.
pub(crate) static CRLF: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"\r\n"));
pub(crate) static CRLF_CRLF: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"\r\n\r\n"));

/// The character encoding declared by the `Content-Type` header's `charset` parameter, or
/// UTF-8 if absent or unparseable.
pub fn encoding(headers: &Headers) -> &'static Encoding {
    headers
        .get_str(KnownHeaderName::ContentType)
        .and_then(|c| Mime::from_str(c).ok())
        .and_then(|m| {
            m.params()
                .find(|(name, _)| name.as_str() == "charset")
                .and_then(|(_, v)| Encoding::for_label(v.as_str().as_bytes()))
        })
        .unwrap_or(encoding_rs::UTF_8)
}

/// Validate and parse an HTTP/1.x `Content-Length` per RFC 9110.
///
/// `values` is the header's value set (`None` if absent). Returns `Ok(None)` when absent and
/// `Ok(Some(len))` for exactly one value that is a non-empty run of ASCII digits fitting in a
/// `u64`.
///
/// Coercing a malformed `Content-Length` to a default body length (zero, or read-to-close) rather
/// than rejecting it is a request/response-smuggling vector, so the server request parser and the
/// client response parser both route through this single check to stay byte-for-byte identical.
///
/// # Errors
///
/// Returns `Err(InvalidHeaderValue)` for more than one value, an empty value, any non-digit octet
/// (notably a leading `+`/`-`, which `u64::from_str` would otherwise accept), or a value that
/// overflows `u64`.
pub fn validate_content_length(values: Option<&HeaderValues>) -> Result<Option<u64>> {
    match values {
        None => Ok(None),
        Some(values) => parse_content_length(values)
            .map(Some)
            .ok_or_else(|| Error::InvalidHeaderValue(KnownHeaderName::ContentLength.into())),
    }
}

/// Parse a `Content-Length` value set into a `u64`, returning `None` for a multi-value set, an
/// empty value, any non-digit octet (notably a leading `+`/`-`, which `u64::from_str` would
/// otherwise accept), or a value that overflows `u64`. Shared by [`validate_content_length`] (the
/// strict ingress gate, which turns this `None` into an error) and [`Headers::content_length`]
/// (the downstream read, which surfaces it directly).
///
/// [`Headers::content_length`]: crate::Headers::content_length
pub(crate) fn parse_content_length(values: &HeaderValues) -> Option<u64> {
    values
        .as_str()
        .filter(|cl| !cl.is_empty() && cl.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|cl| cl.parse::<u64>().ok())
}

/// Whether `b` is a `tchar` — the octets permitted in a `token` (method names, header names,
/// chunk-extension names, etc.).
pub(crate) fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

const ZEROED_WINDOW_FLOOR: usize = 4096;

/// Read `reader` to its end, appending to `buf`.
///
/// `AsyncReadExt::read_to_end` zeroes all of `buf`'s spare capacity before every read, and grows
/// `buf` once more just to observe the end of the stream, so content that exactly fills a
/// preallocation is reallocated at twice the size. This zeroes each byte of capacity at most once,
/// and only as the content arrives, and detects the end through a small probe instead.
///
/// Whether this returns or is dropped, `buf` then holds exactly the bytes read so far.
pub(crate) async fn read_to_end<R>(reader: &mut R, buf: &mut Vec<u8>) -> io::Result<()>
where
    R: AsyncRead + Unpin + ?Sized,
{
    struct Filled<'a> {
        buf: &'a mut Vec<u8>,
        len: usize,
    }

    impl Drop for Filled<'_> {
        fn drop(&mut self) {
            self.buf.truncate(self.len);
        }
    }

    let len = buf.len();
    let mut filled = Filled { buf, len };
    loop {
        if filled.len == filled.buf.len() {
            if filled.buf.len() == filled.buf.capacity() {
                let mut probe = [0; 32];
                let bytes = reader.read(&mut probe).await?;
                if bytes == 0 {
                    return Ok(());
                }
                filled.buf.extend_from_slice(&probe[..bytes]);
                filled.len = filled.buf.len();
                continue;
            }
            // A reader can only fill initialized memory, but zeroing all of a large speculative
            // preallocation for a small body is most of the work. Past a page, the zeroed region
            // doubles as it fills, so the bytes zeroed stay proportional to the bytes received.
            let window = (filled.len * 2)
                .max(ZEROED_WINDOW_FLOOR)
                .min(filled.buf.capacity());
            filled.buf.resize(window, 0);
        }

        let bytes = reader.read(&mut filled.buf[filled.len..]).await?;
        if bytes == 0 {
            return Ok(());
        }
        filled.len += bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::read_to_end;
    use futures_lite::{AsyncRead, future::block_on};
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };

    /// Yields its bytes, then fails.
    struct FailsAfter(&'static [u8]);

    impl AsyncRead for FailsAfter {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            if self.0.is_empty() {
                return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
            }
            let len = buf.len().min(self.0.len());
            buf[..len].copy_from_slice(&self.0[..len]);
            self.0 = &self.0[len..];
            Poll::Ready(Ok(len))
        }
    }

    /// Yields its bytes, recording the size of every buffer it is handed.
    struct RecordsWindows(&'static [u8], Vec<usize>);

    impl AsyncRead for RecordsWindows {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.1.push(buf.len());
            let len = buf.len().min(self.0.len());
            buf[..len].copy_from_slice(&self.0[..len]);
            self.0 = &self.0[len..];
            Poll::Ready(Ok(len))
        }
    }

    #[test]
    fn a_small_body_zeroes_little_of_a_large_preallocation() {
        let mut buf = Vec::with_capacity(64 * 1024);
        let mut reader = RecordsWindows(b"hi", Vec::new());
        block_on(read_to_end(&mut reader, &mut buf)).unwrap();
        assert_eq!(buf, b"hi");
        assert!(
            reader
                .1
                .iter()
                .all(|&window| window <= super::ZEROED_WINDOW_FLOOR),
            "read windows: {:?}",
            reader.1
        );
    }

    #[test]
    fn a_failed_read_leaves_only_the_bytes_read() {
        let mut buf = Vec::with_capacity(64);
        let result = block_on(read_to_end(&mut FailsAfter(b"abc"), &mut buf));
        assert!(result.is_err());
        assert_eq!(buf, b"abc");
    }
}
