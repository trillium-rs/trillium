use super::{
    AsyncRead, AsyncWrite, Buffer, Chunked, Context, End, ErrorKind, Headers, PartialChunkSize,
    Pin, Ready, ReceivedBody, ReceivedBodyState, StateOutput, io, ready, slice_from,
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
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
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
        // Try the buffer first: if the transport is already exhausted but the buffer
        // has enough bytes to complete the header, polling would incorrectly surface
        // `ConnectionAborted`.
        let parse_result = parse_chunk_size(&self.buffer);
        if !matches!(parse_result, Ok(None)) {
            return Ready(interpret_parse_result(
                &mut self.buffer,
                &mut self.trailers,
                parse_result,
                total,
            ));
        }

        let transport = self
            .transport
            .as_deref_mut()
            .ok_or_else(|| io::Error::from(ErrorKind::NotConnected))?;
        let bytes = ready!(Pin::new(transport).poll_read(cx, buf))?;

        if bytes == 0 {
            return Ready(Err(io::Error::from(ErrorKind::ConnectionAborted)));
        }

        self.buffer.extend_from_slice(&buf[..bytes]);

        // The 256-byte cap bounds the chunk-size header line; once parsing succeeds,
        // bytes past the header are legitimate post-header data drained on the next
        // `handle_chunked` pass.
        let parse_result = parse_chunk_size(&self.buffer);
        if matches!(parse_result, Ok(None)) && self.buffer.len() > 256 {
            return Ready(Err(io::Error::new(InvalidData, "chunk header too long")));
        }

        Ready(interpret_parse_result(
            &mut self.buffer,
            &mut self.trailers,
            parse_result,
            total,
        ))
    }
}

/// Translate a `parse_chunk_size` result into the next [`ReceivedBodyState`],
/// advancing `buffer` past the chunk-size header bytes.
fn interpret_parse_result(
    buffer: &mut Buffer,
    trailers: &mut Option<Headers>,
    parse_result: Result<Option<(usize, u64)>, ()>,
    total: u64,
) -> io::Result<(ReceivedBodyState, usize)> {
    match parse_result {
        Ok(Some((used, remaining))) => {
            buffer.ignore_front(used);
            if remaining == 2 {
                // terminal chunk — trailer section begins here (in `buffer`)
                finish_terminal_chunk(buffer, &[], total, trailers)
            } else {
                Ok((Chunked { remaining, total }, 0))
            }
        }
        Ok(None) => Ok((PartialChunkSize { total }, 0)),
        Err(()) => Err(io::Error::new(InvalidData, "invalid chunk size")),
    }
}

impl<Transport> ReceivedBody<'_, Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
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
                    // Terminal chunk — the 2 "consumed" bytes may be real trailer data,
                    // so scan from chunk_start. The remaining buf bytes are earlier in
                    // stream than any residual self_buffer (read_buffered drained
                    // self_buffer's head into buf), so prepend rather than extend.
                    let trailer_start = usize::try_from(chunk_start)
                        .unwrap_or(buf.len())
                        .min(buf.len());
                    self_buffer.prepend(&buf[trailer_start..]);
                    let (state, _) = finish_terminal_chunk(self_buffer, &[], total, trailers)?;
                    break state;
                }
            }

            Ok(None) => {
                // Partial bytes here are earlier in stream than any residual self_buffer
                // (read_buffered drained the head into buf). Prepend, not extend.
                self_buffer.prepend(buf_to_read);
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
    use crate::{Buffer, Headers, HttpConfig};
    use encoding_rs::UTF_8;
    use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, io::Cursor};
    use std::{
        io::{self, Write as _},
        pin::Pin,
        task::{Context, Poll},
    };
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
            HttpConfig::DEFAULT.received_body_max_len,
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

    fn new_with_config(
        input: String,
        config: &HttpConfig,
    ) -> ReceivedBody<'static, Cursor<Vec<u8>>> {
        ReceivedBody::new_with_config(
            None,
            Buffer::from(Vec::with_capacity(config.response_header_initial_capacity)),
            Cursor::new(input.into_bytes()),
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
        decode_with_config(input, poll_size, &HttpConfig::DEFAULT).await
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
        let body = "x".repeat(50);
        let config = HttpConfig::default().with_received_body_max_len(50);
        let result = ReceivedBody::new_with_config(
            Some(50),
            Buffer::default(),
            Cursor::new(body.clone().into_bytes()),
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
            Cursor::new(over.into_bytes()),
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
        // Chunk-size line that is valid so far but never terminates: 16 hex digits +
        // CR, then 300 arbitrary non-LF bytes. The buffer cap (256 bytes) should fire
        // before a complete size line arrives.
        let mut input = "FFFFFFFFFFFFFFFF\r".to_string();
        input.extend(std::iter::repeat_n('x', 300));
        assert!(decode(input, 1).await.is_err());
    }

    #[test(harness)]
    async fn read_string_and_read_bytes() {
        let content = build_chunked_body("test ".repeat(100)).await;
        assert_eq!(
            new_with_config(content.clone(), &HttpConfig::DEFAULT)
                .read_string()
                .await
                .unwrap()
                .len(),
            500
        );

        assert_eq!(
            new_with_config(content.clone(), &HttpConfig::DEFAULT)
                .read_bytes()
                .await
                .unwrap()
                .len(),
            500
        );

        assert!(
            new_with_config(
                content.clone(),
                &HttpConfig::DEFAULT.with_received_body_max_len(400)
            )
            .read_string()
            .await
            .is_err()
        );

        assert!(
            new_with_config(
                content.clone(),
                &HttpConfig::DEFAULT.with_received_body_max_len(400)
            )
            .read_bytes()
            .await
            .is_err()
        );

        assert!(
            new_with_config(content.clone(), &HttpConfig::DEFAULT)
                .with_max_len(400)
                .read_bytes()
                .await
                .is_err()
        );

        assert!(
            new_with_config(content.clone(), &HttpConfig::DEFAULT)
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
            Cursor::new(input.as_bytes().to_vec()),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            &HttpConfig::DEFAULT,
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
                Cursor::new(input.as_bytes().to_vec()),
                ReceivedBodyState::Start,
                None,
                UTF_8,
                &HttpConfig::DEFAULT,
            )
            .with_trailers(&mut trailers);
        }
    }

    #[test(harness)]
    async fn many_tiny_chunks_decode_via_read_to_end() {
        // Repro for the "chunk header too long" false positive in `handle_partial`.
        //
        // The 256-byte cap is supposed to bound the chunk-size header line, but it's
        // applied to `self.buffer` after appending the whole `buf[..bytes]` read from
        // the transport. If `poll_read` is called with a buf larger than ~256 and the
        // wire contains a chain of small chunks, the buffer accumulates valid wire
        // bytes (header + body + more headers) far past the cap before the parser
        // ever runs.
        //
        // `read_bytes()` -> `read_to_end` grows its scratch vec over time, so the
        // per-poll buf size eventually crosses the threshold.
        //
        // Each chunk encodes one body byte as `1\r\nx\r\n` (5 bytes wire per 1 byte
        // body), maximizing header-to-body density to trigger the issue quickly.
        let mut wire = Vec::new();
        for _ in 0..2000 {
            wire.extend_from_slice(b"1\r\nx\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");

        let body = ReceivedBody::new(
            None,
            Buffer::default(),
            Cursor::new(wire),
            ReceivedBodyState::Start,
            None,
            UTF_8,
        );
        let bytes = body
            .read_bytes()
            .await
            .expect("valid chunked wire should decode");
        assert_eq!(bytes.len(), 2000);
        assert!(bytes.iter().all(|&b| b == b'x'));
    }

    #[test(harness)]
    async fn pre_buffered_chunked_with_partial_at_buf_boundary() {
        // Repro for the ordering bug in `chunk_decode`'s `Ok(None)` arm.
        //
        // When `read_buffered` is called with `buf < self.buffer`, it copies
        // `buf.len()` bytes from `self.buffer` to `buf` and leaves the residual
        // (chronologically *later*) bytes in `self.buffer`. `chunk_decode` then
        // processes `buf` and, if a chunk-size header lands at the very end of
        // `buf` as a partial, appends those partial bytes to `self.buffer` via
        // `extend_from_slice`. The partial bytes are chronologically *earlier*
        // than the residual already in `self.buffer`, so the resulting buffer
        // is out of order and subsequent parsing breaks.
        //
        // Construction:
        //   - pre-buffer:  "5\r\nhello\r\n1\r\nx\r"   (16 bytes; ends with partial "1\r")
        //   - transport:   "\n0\r\n\r\n"               (6 bytes; completes the wire)
        //   - user buf:    3 bytes at a time           (forces buf < self.buffer)
        //
        // Full wire end-to-end is the valid chunked body for "hellox" with no
        // trailers, so after the fix this must decode to "hellox".
        let pre_buffer = b"5\r\nhello\r\n1\r\nx\r".to_vec();
        let continuation = b"\n0\r\n\r\n".to_vec();

        let mut rb = ReceivedBody::new(
            None,
            Buffer::from(pre_buffer),
            Cursor::new(continuation),
            ReceivedBodyState::Start,
            None,
            UTF_8,
        );

        let decoded = read_with_buffers_of_size(&mut rb, 3)
            .await
            .expect("valid chunked wire should decode");
        assert_eq!(decoded, "hellox");
    }

    /// `AsyncRead` + `AsyncWrite` over an in-memory `Vec<u8>` with an optional cap on
    /// bytes returned per `poll_read`. `cap = None` means "deliver up to `buf.len()`";
    /// `cap = Some(n)` means "deliver at most `n` bytes per poll, regardless of buf size."
    /// Models transport-level fragmentation patterns that real TCP exhibits.
    struct CappedTransport {
        inner: Cursor<Vec<u8>>,
        cap: Option<usize>,
    }

    impl AsyncRead for CappedTransport {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            let max = self.cap.map_or(buf.len(), |c| c.min(buf.len()));
            Pin::new(&mut self.inner).poll_read(cx, &mut buf[..max])
        }
    }

    impl AsyncWrite for CappedTransport {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_close(cx)
        }
    }

    /// Build a `(body, wire)` pair: `body` is the decoded payload, `wire` is its valid
    /// chunked-encoded representation (no trailers; bare terminator).
    ///
    /// Each chunk's body bytes are derived from `(chunk_index, byte_index)` so that
    /// decoded output can be compared byte-for-byte and corruption surfaces as a
    /// content mismatch rather than just a length mismatch.
    fn build_chunked(chunk_sizes: &[usize]) -> (Vec<u8>, Vec<u8>) {
        let mut body = Vec::new();
        let mut wire = Vec::new();
        for (i, &size) in chunk_sizes.iter().enumerate() {
            let chunk: Vec<u8> = (0..size)
                .map(|j| (i.wrapping_mul(7).wrapping_add(j) & 0xff) as u8)
                .collect();
            body.extend_from_slice(&chunk);
            let _ = write!(wire, "{size:x}\r\n");
            wire.extend_from_slice(&chunk);
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
        (body, wire)
    }

    /// Decode `wire` with `prefill_bytes` of it pre-loaded into the [`Buffer`] (modeling
    /// the conn's pre-read scratch carryover from header parsing) and the remainder
    /// delivered by [`CappedTransport`]. Reads through the [`ReceivedBody`] in
    /// fixed-size slices of `read_size`.
    async fn decode_at_split(
        wire: Vec<u8>,
        prefill_bytes: usize,
        read_size: usize,
        transport_cap: Option<usize>,
    ) -> io::Result<Vec<u8>> {
        let split = prefill_bytes.min(wire.len());
        let prefill = Buffer::from(wire[..split].to_vec());
        let transport = CappedTransport {
            inner: Cursor::new(wire[split..].to_vec()),
            cap: transport_cap,
        };
        let mut rb = ReceivedBody::new(
            None,
            prefill,
            transport,
            ReceivedBodyState::Start,
            None,
            UTF_8,
        );
        let mut out = Vec::new();
        let mut buf = vec![0u8; read_size];
        loop {
            match rb.read(&mut buf).await? {
                0 => return Ok(out),
                n => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    /// Exhaustive round-trip matrix: every combination of body shape × read size ×
    /// transport-delivery cap × prefill split must round-trip cleanly. Designed to
    /// catch regressions in the chunked decoder before they reach production —
    /// adding a constraint that breaks any of these dimensions is the kind of bug
    /// the existing fixed-shape tests historically missed (e.g. the 256-byte cap
    /// regression that needed many tiny chunks + a large read to manifest).
    #[test(harness)]
    async fn round_trip_matrix() {
        let body_shapes: &[&[usize]] = &[
            &[],
            &[1],
            &[5],
            &[100],
            &[1, 1, 1, 1, 1],
            &[1; 10],
            &[1; 100],
            &[1; 2000],
            &[5; 50],
            &[100, 100, 100],
            &[100, 1, 100, 1, 100],
            &[1024; 3],
            &[1, 10, 100, 1000, 100, 10, 1],
        ];
        let read_sizes = [1usize, 2, 3, 5, 17, 64, 128, 256, 257, 1024, 4096];
        let transport_caps: [Option<usize>; 6] =
            [None, Some(1), Some(3), Some(17), Some(257), Some(1024)];

        for shape in body_shapes {
            let (body, wire) = build_chunked(shape);
            // Three prefill modes: nothing pre-buffered, half pre-buffered, fully
            // pre-buffered (modeling the conn's scratch carrying the prologue).
            for split in [0, wire.len() / 2, wire.len()] {
                for &rs in &read_sizes {
                    for &tc in &transport_caps {
                        let decoded = decode_at_split(wire.clone(), split, rs, tc)
                            .await
                            .unwrap_or_else(|e| {
                                panic!(
                                    "decode failed: chunks={shape:?} read_size={rs} \
                                     transport_cap={tc:?} prefill_bytes={split} wire_len={}: {e:?}",
                                    wire.len()
                                )
                            });
                        assert_eq!(
                            decoded, body,
                            "mismatch: chunks={shape:?} read_size={rs} transport_cap={tc:?} \
                             prefill_bytes={split}"
                        );
                    }
                }
            }
        }
    }

    #[test(harness)]
    async fn fully_pre_buffered_body_decodes_with_tiny_reads() {
        // Repro for the third decoder bug: `handle_partial` unconditionally called
        // `transport.poll_read` even when `self.buffer` already had enough bytes to
        // complete the partial chunk-size header. When the transport was exhausted,
        // the 0-byte read tripped the `ConnectionAborted` guard despite the buffer
        // having a perfectly valid chunked stream sitting in it.
        //
        // Reachable in production whenever a small chunked body arrives in the same
        // transport read as its request headers (the conn's pre-read scratch carries
        // the full body) and the user reads with a `buf` small enough that
        // `chunk_decode` can't parse a chunk-size header from a single pass — the
        // remaining bytes get put into `PartialChunkSize` state, then `handle_partial`
        // looked at the transport instead of the buffer and failed.
        //
        // `read_with_buffers_of_size(.., 1)` forces a 1-byte read buffer, which is
        // the minimal trigger.
        let wire = b"5\r\nhello\r\n0\r\n\r\n".to_vec();
        let mut rb = ReceivedBody::new(
            None,
            Buffer::from(wire),
            // Empty transport — every byte must come from the pre-loaded buffer.
            Cursor::new(Vec::<u8>::new()),
            ReceivedBodyState::Start,
            None,
            UTF_8,
        );
        let decoded = read_with_buffers_of_size(&mut rb, 1)
            .await
            .expect("fully-buffered valid chunked wire should decode");
        assert_eq!(decoded, "hello");
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 2048,
            // Persist counterexamples to disk so CI replays known failures first on
            // every run, even after a flaky reseed.
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::SourceParallel(
                    "proptest-regressions",
                ),
            )),
            .. proptest::test_runner::Config::default()
        })]

        /// Property-test counterpart to [`round_trip_matrix`]: instead of an enumerated
        /// matrix, draw random body shapes / read sizes / transport caps / prefill splits
        /// and assert every combination round-trips. Catches the long tail of bug shapes
        /// the hand-picked matrix dimensions don't enumerate.
        #[test]
        fn round_trip_proptest(
            // Chunk sizes start at 1: a 0-size chunk IS the last-chunk marker, so
            // emitting one mid-body would be malformed wire — `build_chunked` would
            // encode it, the decoder would stop early, and the test would
            // false-positive.
            chunk_sizes in proptest::collection::vec(1usize..512, 0..30),
            read_size in 1usize..2048,
            transport_cap in proptest::option::of(1usize..2048),
            prefill_pct in 0u32..=100,
        ) {
            let (body, wire) = build_chunked(&chunk_sizes);
            let prefill_at = (wire.len() * prefill_pct as usize / 100).min(wire.len());
            let decoded = futures_lite::future::block_on(decode_at_split(
                wire,
                prefill_at,
                read_size,
                transport_cap,
            )).map_err(|e| {
                proptest::test_runner::TestCaseError::fail(format!(
                    "decode error: {e:?} chunks={chunk_sizes:?} read_size={read_size} \
                     transport_cap={transport_cap:?} prefill_at={prefill_at}"
                ))
            })?;
            proptest::prop_assert_eq!(
                decoded.len(),
                body.len(),
                "length mismatch (chunks={:?} read_size={} transport_cap={:?} prefill_at={})",
                chunk_sizes, read_size, transport_cap, prefill_at
            );
            proptest::prop_assert_eq!(
                decoded, body,
                "content mismatch (chunks={:?} read_size={} transport_cap={:?} prefill_at={})",
                chunk_sizes, read_size, transport_cap, prefill_at
            );
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
            Cursor::new(input.as_bytes().to_vec()),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            &HttpConfig::DEFAULT,
        )
        .with_trailers(&mut trailers);
        let body = rb.read_string().await.unwrap();
        assert_eq!(body, "hello");
        assert!(trailers.is_none(), "no trailers expected");
    }
}
