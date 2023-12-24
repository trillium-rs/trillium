use super::{
    io, ready, AsyncRead, Context, End, ErrorKind, FixedLength, Ready, ReceivedBody, StateOutput,
};

impl<'conn, Transport> ReceivedBody<'conn, Transport>
where
    Transport: AsyncRead + Unpin + Send + Sync + 'static,
{
    #[inline]
    pub(super) fn handle_fixed_length(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        current_index: u64,
        total_length: u64,
    ) -> StateOutput {
        let len = buf.len();
        let remaining = usize::try_from(total_length - current_index).unwrap_or(usize::MAX);
        let buf = &mut buf[..len.min(remaining)];
        let bytes = ready!(self.read_raw(cx, buf)?);
        let current_index = current_index + bytes as u64;
        if current_index == total_length {
            Ready(Ok((End, bytes)))
        } else if bytes == 0 {
            Ready(Err(io::Error::from(ErrorKind::ConnectionAborted)))
        } else {
            Ready(Ok((
                FixedLength {
                    current_index,
                    total: total_length,
                },
                bytes,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{http_config::DEFAULT_CONFIG, Buffer, HttpConfig, ReceivedBody, ReceivedBodyState};
    use encoding_rs::UTF_8;
    use futures_lite::{future::block_on, io::Cursor, AsyncRead, AsyncReadExt};

    fn new_with_config(
        input: String,
        config: &HttpConfig,
    ) -> ReceivedBody<'static, Cursor<String>> {
        ReceivedBody::new_with_config(
            Some(input.len() as u64),
            Buffer::with_capacity(100),
            Cursor::new(input),
            ReceivedBodyState::Start,
            None,
            UTF_8,
            config,
        )
    }

    fn decode_with_config(
        input: String,
        poll_size: usize,
        config: &HttpConfig,
    ) -> crate::Result<String> {
        let mut rb = new_with_config(input, config);

        block_on(read_with_buffers_of_size(&mut rb, poll_size))
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

    #[test]
    fn test() {
        for size in 3..50 {
            let input = "12345abcdef";
            let output = decode_with_config(input.into(), size, &DEFAULT_CONFIG).unwrap();
            assert_eq!(output, "12345abcdef", "size: {size}");

            let input = "MozillaDeveloperNetwork";
            let output = decode_with_config(input.into(), size, &DEFAULT_CONFIG).unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork", "size: {size}");

            assert!(decode_with_config(String::new(), size, &DEFAULT_CONFIG).is_ok());

            let input = "MozillaDeveloperNetwork";
            assert!(decode_with_config(
                input.into(),
                size,
                &DEFAULT_CONFIG.with_received_body_max_len(5)
            )
            .is_err());
        }
    }

    #[test]
    fn read_string_and_read_bytes() {
        block_on(async {
            let content = "test ".repeat(1000);
            assert_eq!(
                new_with_config(content.clone(), &DEFAULT_CONFIG)
                    .read_string()
                    .await
                    .unwrap()
                    .len(),
                5000
            );

            assert_eq!(
                new_with_config(content.clone(), &DEFAULT_CONFIG)
                    .read_bytes()
                    .await
                    .unwrap()
                    .len(),
                5000
            );

            assert!(new_with_config(
                content.clone(),
                &DEFAULT_CONFIG.with_received_body_max_len(750)
            )
            .read_string()
            .await
            .is_err());

            assert!(new_with_config(
                content.clone(),
                &DEFAULT_CONFIG.with_received_body_max_len(750)
            )
            .read_bytes()
            .await
            .is_err());

            assert!(new_with_config(content.clone(), &DEFAULT_CONFIG)
                .with_max_len(750)
                .read_bytes()
                .await
                .is_err());

            assert!(new_with_config(content.clone(), &DEFAULT_CONFIG)
                .with_max_len(750)
                .read_string()
                .await
                .is_err());
        });
    }
}
