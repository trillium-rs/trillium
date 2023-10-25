use super::{ReceivedBody, ReceivedBodyState};
use crate::{http_config::DEFAULT_CONFIG, HttpConfig};
use encoding_rs::UTF_8;
use futures_lite::{future::block_on, io::Cursor, AsyncRead, AsyncReadExt};

fn new_with_config(input: String, config: &HttpConfig) -> ReceivedBody<'static, Cursor<String>> {
    ReceivedBody::new_with_config(
        Some(input.len() as u64),
        None,
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
