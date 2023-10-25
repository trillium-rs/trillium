use super::{chunk_decode, ReceivedBody, ReceivedBodyState};
use crate::{http_config::DEFAULT_CONFIG, HttpConfig};
use encoding_rs::UTF_8;
use futures_lite::{io::Cursor, AsyncRead, AsyncReadExt};
use trillium_testing::block_on;

fn assert_decoded(
    (remaining, input_data): (u64, &str),
    expected_output: (Option<u64>, &str, Option<&str>),
) {
    let mut buf = input_data.to_string().into_bytes();

    let (output_state, bytes, unused) = chunk_decode(
        remaining,
        0,
        0,
        &mut buf,
        DEFAULT_CONFIG.received_body_max_len,
    )
    .unwrap();

    assert_eq!(
        (
            match output_state {
                ReceivedBodyState::Chunked { remaining, .. } => Some(remaining),
                ReceivedBodyState::End => None,
                _ => panic!("unexpected output state {output_state:?}"),
            },
            &*String::from_utf8_lossy(&buf[0..bytes]),
            unused.as_deref().map(String::from_utf8_lossy).as_deref()
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
        None,
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
        for size in 3..50 {
            let input = "5\r\n12345\r\n1\r\na\r\n2\r\nbc\r\n3\r\ndef\r\n0\r\n";
            let output = decode(input.into(), size).await.unwrap();
            assert_eq!(output, "12345abcdef", "size: {size}");

            let input = "7\r\nMozilla\r\n9\r\nDeveloper\r\n7\r\nNetwork\r\n0\r\n\r\n";
            let output = decode(input.into(), size).await.unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork", "size: {size}");

            assert!(decode(String::new(), size).await.is_err());
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
fn test_read_buffer_too_short() {
    block_on(async {
        let input = "test ".repeat(50);
        let chunked = build_chunked_body(input.clone()).await;
        assert!(chunked.starts_with("FA\r\n"));

        for size in 1..4 {
            assert!(decode(chunked.clone(), size).await.is_err());
        }

        for size in 4..10 {
            assert_eq!(&decode(chunked.clone(), size).await.unwrap(), &input);
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
    assert_decoded((0, "5\r\n12345\r\n"), (Some(0), "12345", None));
    assert_decoded((0, "F\r\n1"), (Some(14 + 2), "1", None));
    assert_decoded((0, "5\r\n123"), (Some(2 + 2), "123", None));
    assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n"), (Some(0), "XX", None));
    assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n1"), (Some(0), "XX", Some("1")));
    assert_decoded((0, "FFF\r\n"), (Some(0xfff + 2), "", None));
    assert_decoded((10, "hello"), (Some(5), "hello", None));
    assert_decoded(
        (7, "hello\r\nA\r\n world"),
        (Some(4 + 2), "hello world", None),
    );
    assert_decoded(
        (0, "e\r\ntest test test\r\n0\r\n\r\n"),
        (None, "test test test", None),
    );
    assert_decoded(
        (0, "1\r\n_\r\n0\r\n\r\nnext request"),
        (None, "_", Some("next request")),
    );
    assert_decoded((7, "hello\r\n0\r\n\r\n"), (None, "hello", None));
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
