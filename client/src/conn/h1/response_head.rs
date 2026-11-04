use futures_lite::{AsyncRead, AsyncReadExt};
use memchr::memmem::Finder;
use std::sync::LazyLock;
use trillium_http::{Buffer, Error, Headers, HttpConfig, Result, Version};

// A `Finder` plans its search strategy on construction, which is wasted work to repeat per parse.
static CRLF: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"\r\n"));
static CRLF_CRLF: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"\r\n\r\n"));

/// The status line and header fields of an HTTP/1.x response head.
pub(crate) struct ResponseHead {
    pub(crate) version: Version,
    /// The three-digit status code, which [`Status`](trillium_http::Status) may not represent.
    pub(crate) code: u16,
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
    let line_len = CRLF.find(head).ok_or(Error::InvalidHead)?;
    let (version, code) = parse_status_line(&head[..line_len])?;
    let headers = Headers::parse(&head[line_len + 2..]).map_err(|_| Error::InvalidHead)?;
    Ok(ResponseHead {
        version,
        code,
        headers,
    })
}

/// `HTTP/1.x SP 3DIGIT [SP reason-phrase]` (RFC 9112 §4). A minor version above 1 is read as
/// HTTP/1.1 (RFC 9110 §2.5). The space before an empty reason phrase is optional, since servers
/// commonly omit it.
fn parse_status_line(line: &[u8]) -> Result<(Version, u16)> {
    let version = match line.get(..9) {
        Some(b"HTTP/1.0 ") => Version::Http1_0,
        Some([b'H', b'T', b'T', b'P', b'/', b'1', b'.', minor, b' ']) if minor.is_ascii_digit() => {
            Version::Http1_1
        }
        _ => return Err(Error::InvalidHead),
    };

    let code = match line.get(9..12) {
        Some(&[a, b, c]) if [a, b, c].iter().all(u8::is_ascii_digit) => {
            u16::from(a - b'0') * 100 + u16::from(b - b'0') * 10 + u16::from(c - b'0')
        }
        _ => return Err(Error::InvalidStatus),
    };

    let reason = match &line[12..] {
        [] => &[][..],
        [b' ', reason @ ..] => reason,
        _ => return Err(Error::InvalidHead),
    };
    if reason
        .iter()
        .any(|&byte| (byte < b' ' && byte != b'\t') || byte == 0x7f)
    {
        return Err(Error::InvalidHead);
    }

    Ok((version, code))
}
