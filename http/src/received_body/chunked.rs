use super::{
    AsyncRead, Buffer, Chunked, Context, End, ErrorKind, Headers, PartialChunkSize, Pin, Ready,
    ReceivedBody, ReceivedBodyState, StateOutput, io, ready, slice_from,
};
use std::io::ErrorKind::InvalidData;

#[cfg(feature = "parse")]
fn parse_chunk_size(buf: &[u8]) -> Result<Option<(usize, u64)>, ()> {
    use memchr::memmem::Finder;
    use std::str;

    let Some(index) = memchr::memchr2(b';', b'\r', &buf[..buf.len().min(17)]) else {
        return if buf.len() < 17 { Ok(None) } else { Err(()) };
    };
    let src = str::from_utf8(&buf[..index]).map_err(|_| ())?;
    let chunk_size = u64::from_str_radix(src, 16).map_err(|_| ())?;
    Ok(Finder::new("\r\n")
        .find(&buf[index..])
        .map(|end| (index + end + 2, chunk_size + 2)))
}

#[cfg(not(feature = "parse"))]
fn parse_chunk_size(buf: &[u8]) -> Result<Option<(usize, u64)>, ()> {
    use httparse::{Status, parse_chunk_size};
    match parse_chunk_size(buf) {
        Ok(Status::Complete((index, next_chunk))) => Ok(Some((index, next_chunk + 2))),
        Ok(Status::Partial) => Ok(None),
        Err(_) => Err(()),
    }
}

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    #[inline]
    pub(super) fn handle_chunked(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        remaining: u64,
        total: u64,
    ) -> StateOutput {
        let bytes = ready!(self.read_raw(cx, buf)?);

        Ready(chunk_decode(
            &mut self.buffer,
            remaining,
            total,
            &mut buf[..bytes],
            self.max_len,
            &mut self.trailers,
        ))
    }

    #[inline]
    pub(super) fn handle_partial(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        total: u64,
    ) -> StateOutput {
        let transport = self
            .transport
            .as_deref_mut()
            .ok_or_else(|| io::Error::from(ErrorKind::NotConnected))?;
        let bytes = ready!(Pin::new(transport).poll_read(cx, buf))?;

        if bytes == 0 {
            return Ready(Err(io::Error::from(ErrorKind::ConnectionAborted)));
        }

        self.buffer.extend_from_slice(&buf[..bytes]);

        if self.buffer.len() > 256 {
            return Ready(Err(io::Error::new(InvalidData, "chunk header too long")));
        }

        Ready(match parse_chunk_size(&self.buffer) {
            Ok(Some((used, remaining))) => {
                self.buffer.ignore_front(used);
                if remaining == 2 {
                    // terminal chunk — trailer section begins here (in self.buffer)
                    finish_terminal_chunk(&mut self.buffer, &[], total, &mut self.trailers)
                } else {
                    Ok((Chunked { remaining, total }, 0))
                }
            }
            Ok(None) => Ok((PartialChunkSize { total }, 0)),
            Err(()) => Err(io::Error::new(InvalidData, "invalid chunk size")),
        })
    }
}

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    /// Handler for [`ReceivedBodyState::ReadingH1Trailers`].
    ///
    /// We're past the terminal `0\r\n` and need to collect the trailer-section + final CRLF.
    /// Partial bytes accumulate in `self.buffer`; once a complete trailer terminator is found the
    /// trailers are decoded and we transition to `End`.
    #[inline]
    pub(super) fn handle_reading_h1_trailers(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        total: u64,
    ) -> StateOutput {
        let transport = self
            .transport
            .as_deref_mut()
            .ok_or_else(|| io::Error::from(ErrorKind::NotConnected))?;
        let bytes = ready!(Pin::new(transport).poll_read(cx, buf))?;

        if bytes == 0 {
            return Ready(Err(io::Error::from(ErrorKind::UnexpectedEof)));
        }

        Ready(finish_terminal_chunk(
            &mut self.buffer,
            &buf[..bytes],
            total,
            &mut self.trailers,
        ))
    }
}

pub(super) fn chunk_decode(
    self_buffer: &mut Buffer,
    remaining: u64,
    mut total: u64,
    buf: &mut [u8],
    max_len: u64,
    trailers: &mut Option<Headers>,
) -> io::Result<(ReceivedBodyState, usize)> {
    if buf.is_empty() {
        return Err(io::Error::from(ErrorKind::ConnectionAborted));
    }
    let mut ranges_to_keep = vec![];
    let mut chunk_start = 0u64;
    let mut chunk_end = remaining;
    let request_body_state = loop {
        if chunk_end > 2 {
            let keep_start = usize::try_from(chunk_start).unwrap_or(usize::MAX);
            let keep_end = buf
                .len()
                .min(usize::try_from(chunk_end - 2).unwrap_or(usize::MAX));
            ranges_to_keep.push(keep_start..keep_end);
            let new_bytes = (keep_end - keep_start) as u64;
            total += new_bytes;
            if total > max_len {
                return Err(io::Error::new(ErrorKind::Unsupported, "content too long"));
            }
        }
        chunk_start = chunk_end;

        let Some(buf_to_read) = slice_from(chunk_start, buf) else {
            break Chunked {
                remaining: (chunk_start - buf.len() as u64),
                total,
            };
        };

        if buf_to_read.is_empty() {
            break Chunked {
                remaining: (chunk_start - buf.len() as u64),
                total,
            };
        }

        match parse_chunk_size(buf_to_read) {
            Ok(Some((framing_bytes, chunk_size))) => {
                chunk_start += framing_bytes as u64;
                chunk_end = chunk_start
                    .checked_add(chunk_size)
                    .ok_or_else(|| io::Error::new(InvalidData, "chunk size too long"))?;

                if chunk_size == 2 {
                    // terminal chunk — `chunk_start` is the start of the trailer-section.
                    // The 2 bytes already "consumed" by chunk_size may be real trailer data,
                    // so scan from chunk_start (not chunk_end) for the trailer terminator.
                    let trailer_start = usize::try_from(chunk_start)
                        .unwrap_or(buf.len())
                        .min(buf.len());
                    let (state, _) =
                        finish_terminal_chunk(self_buffer, &buf[trailer_start..], total, trailers)?;
                    break state;
                }
            }

            Ok(None) => {
                self_buffer.extend_from_slice(buf_to_read);
                break PartialChunkSize { total };
            }

            Err(()) => {
                return Err(io::Error::new(InvalidData, "invalid chunk size"));
            }
        }
    };

    let mut bytes = 0;

    for range_to_keep in ranges_to_keep {
        let new_bytes = bytes + range_to_keep.end - range_to_keep.start;
        buf.copy_within(range_to_keep, bytes);
        bytes = new_bytes;
    }

    Ok((request_body_state, bytes))
}

/// Called when we've just read the terminal last-chunk (`0\r\n`). `trailer_bytes` contains
/// whatever bytes from the current read buffer follow the `0\r\n`.
///
/// Looks for the end of the trailer-section (a bare `\r\n` for no trailers, or `\r\n\r\n`
/// after one or more header fields). Returns `(End, 0)` when complete, or
/// `(ReadingH1Trailers { total }, 0)` when we need more bytes.
///
/// Bytes that belong to the next request (after the trailer terminator) are placed in
/// `self_buffer`.
fn finish_terminal_chunk(
    self_buffer: &mut Buffer,
    trailer_bytes: &[u8],
    total: u64,
    trailers: &mut Option<Headers>,
) -> io::Result<(ReceivedBodyState, usize)> {
    // Combine any previously buffered partial trailer bytes with the new bytes.
    let combined: Vec<u8> = if self_buffer.is_empty() {
        trailer_bytes.to_vec()
    } else {
        let mut v = self_buffer.to_vec();
        v.extend_from_slice(trailer_bytes);
        self_buffer.truncate(0);
        v
    };

    if let Some((trailer_header_end, consumed)) = find_trailer_end(&combined) {
        if trailer_header_end > 0 {
            *trailers = Some(parse_h1_trailers(&combined[..trailer_header_end])?);
        }
        // anything after the trailer terminator is the start of the next request
        let leftover = &combined[consumed..];
        if !leftover.is_empty() {
            self_buffer.extend_from_slice(leftover);
        }
        Ok((End, 0))
    } else {
        // Need more bytes — stash what we have and wait
        self_buffer.extend_from_slice(&combined);
        Ok((ReceivedBodyState::ReadingH1Trailers { total }, 0))
    }
}

/// Returns `Some((trailer_header_end, consumed))` when the end of the trailer-section is found
/// in `bytes` (bytes starting right after the terminal `0\r\n`):
///
/// - `trailer_header_end`: how many bytes of actual trailer headers are present (0 = no trailers)
/// - `consumed`: how many total bytes to consume (includes the terminating CRLF)
///
/// Returns `None` if more bytes are needed.
fn find_trailer_end(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() >= 2 && bytes.starts_with(b"\r\n") {
        // No trailers — just the empty-line terminator
        Some((0, 2))
    } else {
        // Trailers present — look for \r\n\r\n (last header's CRLF + empty line)
        memchr::memmem::find(bytes, b"\r\n\r\n").map(|pos| (pos + 2, pos + 4))
    }
}

/// Parse HTTP/1.1 trailer header bytes into a [`Headers`] value.
///
/// `bytes` must contain only the header fields, each terminated with `\r\n`, with no leading
/// or trailing empty line (e.g. `"Name: Value\r\nOther: X\r\n"`).
fn parse_h1_trailers(bytes: &[u8]) -> io::Result<Headers> {
    #[cfg(feature = "parse")]
    {
        Headers::parse(bytes).map_err(|_| io::Error::new(InvalidData, "invalid trailer headers"))
    }

    #[cfg(not(feature = "parse"))]
    {
        use crate::{HeaderName, HeaderValue};
        use std::str::FromStr;
        const MAX_HEADERS: usize = 64;

        // httparse::parse_headers expects the header section to be terminated by a blank line
        // (\r\n\r\n). Our `bytes` contains only the field lines (each ending with \r\n) with no
        // terminating blank line, so we append one before handing off to httparse.
        let mut input = bytes.to_vec();
        input.extend_from_slice(b"\r\n");

        let mut raw = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut headers = Headers::new();
        match httparse::parse_headers(&input, &mut raw) {
            Ok(httparse::Status::Complete((_, parsed))) => {
                for h in parsed {
                    if h.name.is_empty() {
                        break;
                    }
                    let name = HeaderName::from_str(h.name)
                        .map_err(|_| io::Error::new(InvalidData, "invalid trailer header name"))?;
                    let value = HeaderValue::from(h.value.to_owned());
                    headers.append(name, value);
                }
                Ok(headers)
            }
            _ => Err(io::Error::new(InvalidData, "invalid trailer headers")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReceivedBody, ReceivedBodyState, chunk_decode};
    use crate::{Buffer, Headers, HttpConfig, http_config::DEFAULT_CONFIG};
    use encoding_rs::UTF_8;
    use futures_lite::{AsyncRead, AsyncReadExt, io::Cursor};
    use test_harness::test;
    use trillium_testing::harness;

    #[track_caller]
    fn assert_decoded(
        (remaining, input_data): (u64, &str),
        expected_output: (Option<u64>, &str, &str),
    ) {
        let mut buf = input_data.to_string().into_bytes();
        let mut self_buf = Buffer::with_capacity(100);

        let (output_state, bytes) = chunk_decode(
            &mut self_buf,
            remaining,
            0,
            &mut buf,
            DEFAULT_CONFIG.received_body_max_len,
            &mut None,
        )
        .unwrap();

        assert_eq!(
            (
                match output_state {
                    ReceivedBodyState::Chunked { remaining, .. } => Some(remaining),
                    ReceivedBodyState::PartialChunkSize { .. } => Some(0),
                    ReceivedBodyState::End => None,
                    _ => panic!("unexpected output state {output_state:?}"),
                },
                &*String::from_utf8_lossy(&buf[0..bytes]),
                &*String::from_utf8_lossy(&self_buf)
            ),
            expected_output
        );
    }

    async fn read_with_buffers_of_size<R>(reader: &mut R, size: usize) -> crate::Result<String>
    where
        R: AsyncRead + Unpin,
    {
        let mut return_buffer = vec![];
        loop {
            let mut buf = vec![0; size];
            match reader.read(&mut buf).await? {
                0 => break Ok(String::from_utf8_lossy(&return_buffer).into()),
                bytes_read => return_buffer.extend_from_slice(&buf[..bytes_read]),
            }
        }
    }

    fn new_with_config(input: String, config: &HttpConfig) -> ReceivedBody<'_, Cursor<String>> {
        ReceivedBody::new_with_config(
            None,
            Buffer::from(Vec::with_capacity(config.response_header_initial_capacity)),
            Cursor::new(input),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            config,
        )
    }

    async fn decode_with_config(
        input: String,
        poll_size: usize,
        config: &HttpConfig,
    ) -> crate::Result<String> {
        let mut rb = new_with_config(input, config);
        read_with_buffers_of_size(&mut rb, poll_size).await
    }

    async fn decode(input: String, poll_size: usize) -> crate::Result<String> {
        decode_with_config(input, poll_size, &DEFAULT_CONFIG).await
    }

    #[test(harness)]
    async fn test_full_decode() {
        for size in 1..50 {
            let input = "5\r\n12345\r\n1\r\na\r\n2\r\nbc\r\n3\r\ndef\r\n0\r\n\r\n";
            let output = decode(input.into(), size).await.unwrap();
            assert_eq!(output, "12345abcdef", "size: {size}");

            let input = "7\r\nMozilla\r\n9\r\nDeveloper\r\n7\r\nNetwork\r\n0\r\n\r\n";
            let output = decode(input.into(), size).await.unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork", "size: {size}");

            assert!(decode(String::new(), size).await.is_err());
            assert!(decode("fffffffffffffff0\r\n".into(), size).await.is_err());
        }
    }

    async fn build_chunked_body(input: String) -> String {
        let mut output = Vec::with_capacity(10);
        let len = crate::copy(
            crate::Body::new_streaming(Cursor::new(input), None),
            &mut output,
            16,
        )
        .await
        .unwrap();

        output.truncate(len.try_into().unwrap());
        // The last-chunk (`0\r\n`) is emitted by Body::poll_read; the caller (server/client
        // send path) appends the trailer-section terminator. Mimic that here.
        output.extend_from_slice(b"\r\n");
        String::from_utf8(output).unwrap()
    }

    #[test(harness)]
    async fn test_read_buffer_short() {
        let input = "test ".repeat(50);
        let chunked = build_chunked_body(input.clone()).await;

        for size in 1..10 {
            assert_eq!(
                &decode(chunked.clone(), size).await.unwrap(),
                &input,
                "size: {size}"
            );
        }
    }

    #[test(harness)]
    async fn test_max_len() {
        let input = build_chunked_body("test ".repeat(10)).await;

        for size in 4..10 {
            assert!(
                decode_with_config(
                    input.clone(),
                    size,
                    &HttpConfig::default().with_received_body_max_len(5)
                )
                .await
                .is_err()
            );

            assert!(
                decode_with_config(input.clone(), size, &HttpConfig::default())
                    .await
                    .is_ok()
            );
        }
    }

    #[test]
    fn test_chunk_start() {
        let _ = env_logger::builder().is_test(true).try_init();
        assert_decoded((0, "5\r\n12345\r\n"), (Some(0), "12345", ""));
        assert_decoded((0, "F\r\n1"), (Some(14 + 2), "1", ""));
        assert_decoded((0, "5\r\n123"), (Some(2 + 2), "123", ""));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n"), (Some(0), "XX", ""));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n1"), (Some(0), "XX", "1"));
        assert_decoded((0, "FFF\r\n"), (Some(0xfff + 2), "", ""));
        assert_decoded((10, "hello"), (Some(5), "hello", ""));
        assert_decoded(
            (7, "hello\r\nA\r\n world"),
            (Some(4 + 2), "hello world", ""),
        );
        assert_decoded(
            (0, "e\r\ntest test test\r\n0\r\n\r\n"),
            (None, "test test test", ""),
        );
        assert_decoded(
            (0, "1\r\n_\r\n0\r\n\r\nnext request"),
            (None, "_", "next request"),
        );
        assert_decoded((7, "hello\r\n0\r\n\r\n"), (None, "hello", ""));
    }

    #[test]
    fn test_chunk_start_with_ext() {
        let _ = env_logger::builder().is_test(true).try_init();

        assert_decoded((0, "5;abcdefg\r\n12345\r\n"), (Some(0), "12345", ""));
        assert_decoded((0, "F;aaa\taaaaa\taaa aaa\r\n1"), (Some(14 + 2), "1", ""));
        assert_decoded((0, "5;;;;;;;;;;;;;;;;\r\n123"), (Some(2 + 2), "123", ""));
        assert_decoded(
            (0, "1;   a = b\"\" \r\nX\r\n1;;;\r\nX\r\n"),
            (Some(0), "XX", ""),
        );
        assert_decoded((0, "1\r\nX\r\n1;\r\nX\r\n1"), (Some(0), "XX", "1"));
        assert_decoded((0, "FFF; 000\r\n"), (Some(0xfff + 2), "", ""));
        assert_decoded((10, "hello"), (Some(5), "hello", ""));
        assert_decoded(
            (7, "hello\r\nA;111\r\n world"),
            (Some(4 + 2), "hello world", ""),
        );
        assert_decoded(
            (0, "e\r\ntest test test\r\n0;00\r\n\r\n"),
            (None, "test test test", ""),
        );
        assert_decoded(
            (0, "1;\r\n_\r\n0;\r\n\r\nnext request"),
            (None, "_", "next request"),
        );
        assert_decoded((7, "hello\r\n0;\r\n\r\n"), (None, "hello", ""));
    }

    #[test(harness)]
    async fn test_fixed_length_exactly_at_max_len() {
        // A body of exactly max_len bytes must succeed (not be rejected).
        // Previously handle_start used `<` instead of `<=`, so a body of exactly
        // max_len was incorrectly rejected before reading a single byte.
        let body = "x".repeat(50);
        let config = HttpConfig::default().with_received_body_max_len(50);
        let result = ReceivedBody::new_with_config(
            Some(50),
            Buffer::default(),
            Cursor::new(body.clone()),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            &config,
        )
        .read_string()
        .await;
        assert!(
            result.is_ok(),
            "exact-max-len body should succeed: {result:?}"
        );
        assert_eq!(result.unwrap(), body);

        // One byte over must still fail.
        let over = "x".repeat(51);
        let err = ReceivedBody::new_with_config(
            Some(51),
            Buffer::default(),
            Cursor::new(over),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            &config,
        )
        .read_string()
        .await;
        assert!(err.is_err(), "over-max-len body should fail");
    }

    #[test(harness)]
    async fn test_chunk_header_too_long() {
        // Build a chunk size line that is valid so far but never terminates:
        // 16 hex digits + CR, then 300 arbitrary non-LF bytes.
        // The buffer cap (256 bytes) should fire before we ever see a complete
        // size line, returning an error rather than growing forever.
        // 16 hex digits + CR, then 300 arbitrary non-LF bytes. The buffer cap
        // (256 bytes) should fire before a complete size line arrives.
        let mut input = "FFFFFFFFFFFFFFFF\r".to_string();
        input.extend(std::iter::repeat('x').take(300));
        assert!(decode(input, 1).await.is_err());
    }

    #[test(harness)]
    async fn read_string_and_read_bytes() {
        let content = build_chunked_body("test ".repeat(100)).await;
        assert_eq!(
            new_with_config(content.clone(), &DEFAULT_CONFIG)
                .read_string()
                .await
                .unwrap()
                .len(),
            500
        );

        assert_eq!(
            new_with_config(content.clone(), &DEFAULT_CONFIG)
                .read_bytes()
                .await
                .unwrap()
                .len(),
            500
        );

        assert!(
            new_with_config(
                content.clone(),
                &DEFAULT_CONFIG.with_received_body_max_len(400)
            )
            .read_string()
            .await
            .is_err()
        );

        assert!(
            new_with_config(
                content.clone(),
                &DEFAULT_CONFIG.with_received_body_max_len(400)
            )
            .read_bytes()
            .await
            .is_err()
        );

        assert!(
            new_with_config(content.clone(), &DEFAULT_CONFIG)
                .with_max_len(400)
                .read_bytes()
                .await
                .is_err()
        );

        assert!(
            new_with_config(content.clone(), &DEFAULT_CONFIG)
                .with_max_len(400)
                .read_string()
                .await
                .is_err()
        );
    }

    #[test(harness)]
    async fn trailers_decoded_into_destination() {
        let input = "5\r\nhello\r\n0\r\nx-checksum: abc123\r\n\r\n";
        let mut trailers: Option<Headers> = None;
        let mut rb = ReceivedBody::new_with_config(
            None,
            Buffer::default(),
            Cursor::new(input),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            &DEFAULT_CONFIG,
        )
        .with_trailers(&mut trailers);

        for size in [1, 3, 7, 32, 256] {
            let body = read_with_buffers_of_size(&mut rb, size).await.unwrap();
            assert_eq!(body, "hello", "size={size}");
            let t = trailers.take().expect("trailers should be populated");
            assert_eq!(t.get_str("x-checksum"), Some("abc123"), "size={size}");

            // reset for next iteration
            rb = ReceivedBody::new_with_config(
                None,
                Buffer::default(),
                Cursor::new(input),
                ReceivedBodyState::Start,
                None,
                UTF_8,
                &DEFAULT_CONFIG,
            )
            .with_trailers(&mut trailers);
        }
    }

    #[test(harness)]
    async fn trailers_with_no_trailers_section() {
        // Body with no trailers — just the bare empty-line terminator
        let input = "5\r\nhello\r\n0\r\n\r\n";
        let mut trailers: Option<Headers> = None;
        let rb = ReceivedBody::new_with_config(
            None,
            Buffer::default(),
            Cursor::new(input),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            &DEFAULT_CONFIG,
        )
        .with_trailers(&mut trailers);
        let body = rb.read_string().await.unwrap();
        assert_eq!(body, "hello");
        assert!(trailers.is_none(), "no trailers expected");
    }
}
