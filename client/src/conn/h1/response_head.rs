use futures_lite::{AsyncRead, AsyncReadExt};
use memchr::memmem::Finder;
use std::{str, sync::LazyLock};
use trillium_http::{Buffer, Error, Headers, HttpConfig, Result, Status, Version};

// A `Finder` plans its search strategy on construction, which is wasted work to repeat per parse.
static CRLF: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"\r\n"));
static CRLF_CRLF: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"\r\n\r\n"));

/// The status line and header fields of an HTTP/1.x response head.
pub(crate) struct ResponseHead {
    pub(crate) version: Version,
    pub(crate) status: Status,
    pub(crate) headers: Headers,
}

/// Reads from `transport` into `buffer` until the buffer holds a complete response head, and
/// returns the head's length including the terminating blank line. Bytes received past the head
/// remain in `buffer`, after it.
pub(crate) async fn read_head<T>(
    transport: &mut T,
    buffer: &mut Buffer,
    config: &HttpConfig,
) -> Result<usize>
where
    T: AsyncRead + Unpin + ?Sized,
{
    // Keepalive boundary: release window space beyond the high-water mark so
    // bytes from an earlier response can't be re-lent while parsing this
    // one. Live bytes survive (capacity is retained and re-zeroed on
    // growth), including any head already buffered by a fast path below.
    buffer.truncate();

    let head_max_len = config.head_max_len();
    // a zero request_buffer_initial_len must not produce a zero-length window,
    // which would misread as connection closed
    let read_floor = config.request_buffer_initial_len().max(1);

    let mut len = buffer.live_len();
    let mut search_start = 0;

    if len > 0 {
        if let Some(index) = CRLF_CRLF.find(buffer) {
            return Ok(index + 4);
        }
        search_start = len.saturating_sub(3);
    }

    loop {
        // Checked before reading: the window below cannot exceed the
        // remaining allowance, so a peer pacing a response head one byte at
        // a time can never drive the buffer past head_max_len. Within
        // that ceiling, lending grows geometrically with received bytes.
        if len >= head_max_len {
            return Err(Error::HeadersTooLong);
        }

        let want = (head_max_len - len).min(read_floor.max(len * 2));
        let bytes = transport.read(buffer.window(want)).await?;
        buffer.advance(bytes);
        len += bytes;

        if let Some(index) = CRLF_CRLF.find(&buffer[search_start..len]) {
            return Ok(search_start + index + 4);
        }

        search_start = len.saturating_sub(3);

        if bytes == 0 {
            return Err(if len == 0 {
                Error::Closed
            } else {
                Error::InvalidHead
            });
        }
    }
}

/// Parses a complete response head, as measured by [`read_head`].
pub(crate) fn parse_head(head: &[u8]) -> Result<ResponseHead> {
    let space = memchr::memchr(b' ', head).ok_or(Error::InvalidHead)?;
    let version = str::from_utf8(&head[..space])
        .map_err(|_| Error::InvalidHead)?
        .parse()
        .map_err(|_| Error::InvalidHead)?;
    let status = str::from_utf8(&head[space + 1..space + 4])?.parse()?;
    // The status-code is exactly three digits; the next octet must terminate it —
    // a SP before the reason-phrase, or the CR/LF ending the status-line. Reject a 4th digit so
    // e.g. `2000` isn't silently truncated to `200`.
    if !matches!(head.get(space + 4), Some(b' ' | b'\r' | b'\n')) {
        return Err(Error::InvalidHead);
    }
    let end_of_first_line = 2 + CRLF.find(head).ok_or(Error::InvalidHead)?;
    let headers = Headers::parse(&head[end_of_first_line..]).map_err(|_| Error::InvalidHead)?;
    Ok(ResponseHead {
        version,
        status,
        headers,
    })
}
