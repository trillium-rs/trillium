use super::{
    io, ready, slice_from, AsyncRead, Chunked, Context, End, ErrorKind, InvalidChunkSize,
    PartialChunkSize, Pin, Ready, ReceivedBody, ReceivedBodyState, StateOutput, Status,
};

impl<'conn, Transport> ReceivedBody<'conn, Transport>
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

        match httparse::parse_chunk_size(&self.buffer) {
            Ok(Status::Complete((framing_bytes, remaining))) => {
                let len = self.buffer.len() - framing_bytes;
                self.buffer.copy_within(framing_bytes.., 0);
                self.buffer.truncate(len);

                Ready(Ok((
                    if remaining == 0 {
                        End
                    } else {
                        Chunked {
                            remaining: remaining + 2,
                            total,
                        }
                    },
                    0,
                )))
            }

            Ok(Status::Partial) => Ready(Ok((PartialChunkSize { total }, 0))),

            Err(InvalidChunkSize) => Ready(Err(io::Error::new(
                ErrorKind::InvalidData,
                "invalid chunk framing",
            ))),
        }
    }
}

pub(super) fn chunk_decode(
    self_buffer: &mut Vec<u8>,
    remaining: u64,
    mut total: u64,
    buf: &mut [u8],
    max_len: u64,
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

        match httparse::parse_chunk_size(buf_to_read) {
            Ok(Status::Complete((framing_bytes, chunk_size))) => {
                chunk_start += framing_bytes as u64;
                chunk_end = (2 + chunk_start)
                    .checked_add(chunk_size)
                    .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "chunk size too long"))?;

                if chunk_size == 0 {
                    if let Some(buf) = slice_from(chunk_end, buf) {
                        self_buffer.extend_from_slice(buf);
                    }
                    break End;
                }
            }

            Ok(Status::Partial) => {
                self_buffer.extend_from_slice(buf_to_read);
                break PartialChunkSize { total };
            }

            Err(InvalidChunkSize) => {
                return Err(io::Error::new(ErrorKind::InvalidData, "invalid chunk size"));
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

#[cfg(test)]
mod tests {
    use super::{chunk_decode, ReceivedBody, ReceivedBodyState};
    use crate::{http_config::DEFAULT_CONFIG, HttpConfig};
    use encoding_rs::UTF_8;
    use futures_lite::{io::Cursor, AsyncRead, AsyncReadExt};
    use trillium_testing::block_on;

    #[track_caller]
    fn assert_decoded(
        (remaining, input_data): (u64, &str),
        expected_output: (Option<u64>, &str, &str),
    ) {
        let mut buf = input_data.to_string().into_bytes();
        let mut self_buf = Vec::with_capacity(100);

        let (output_state, bytes) = chunk_decode(
            &mut self_buf,
            remaining,
            0,
            &mut buf,
            DEFAULT_CONFIG.received_body_max_len,
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
            Vec::with_capacity(100),
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

    #[test]
    fn test_full_decode() {
        block_on(async {
            for size in 1..50 {
                let input = "5\r\n12345\r\n1\r\na\r\n2\r\nbc\r\n3\r\ndef\r\n0\r\n";
                let output = decode(input.into(), size).await.unwrap();
                assert_eq!(output, "12345abcdef", "size: {size}");

                let input = "7\r\nMozilla\r\n9\r\nDeveloper\r\n7\r\nNetwork\r\n0\r\n\r\n";
                let output = decode(input.into(), size).await.unwrap();
                assert_eq!(output, "MozillaDeveloperNetwork", "size: {size}");

                assert!(decode(String::new(), size).await.is_err());
                assert!(decode("fffffffffffffff0\r\n".into(), size).await.is_err());
            }
        });
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
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_read_buffer_short() {
        block_on(async {
            let input = "test ".repeat(50);
            let chunked = build_chunked_body(input.clone()).await;

            for size in 1..10 {
                assert_eq!(
                    &decode(chunked.clone(), size).await.unwrap(),
                    &input,
                    "size: {size}"
                );
            }
        });
    }

    #[test]
    fn test_max_len() {
        block_on(async {
            let input = build_chunked_body("test ".repeat(10)).await;

            for size in 4..10 {
                assert!(decode_with_config(
                    input.clone(),
                    size,
                    &HttpConfig::default().with_received_body_max_len(5)
                )
                .await
                .is_err());

                assert!(
                    decode_with_config(input.clone(), size, &HttpConfig::default())
                        .await
                        .is_ok()
                );
            }
        });
    }

    #[test]
    fn test_chunk_start() {
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
    fn read_string_and_read_bytes() {
        block_on(async {
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

            assert!(new_with_config(
                content.clone(),
                &DEFAULT_CONFIG.with_received_body_max_len(400)
            )
            .read_string()
            .await
            .is_err());

            assert!(new_with_config(
                content.clone(),
                &DEFAULT_CONFIG.with_received_body_max_len(400)
            )
            .read_bytes()
            .await
            .is_err());

            assert!(new_with_config(content.clone(), &DEFAULT_CONFIG)
                .with_max_len(400)
                .read_bytes()
                .await
                .is_err());

            assert!(new_with_config(content.clone(), &DEFAULT_CONFIG)
                .with_max_len(400)
                .read_string()
                .await
                .is_err());
        });
    }
}
