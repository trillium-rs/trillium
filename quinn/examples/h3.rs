use futures_lite::{AsyncRead, io::Cursor};
use std::{
    io::Result,
    pin::Pin,
    process::ExitCode,
    task::{Context, Poll},
};
use trillium::{Body, BodySource, Conn, Headers, KnownHeaderName};
use trillium_client::Url;
use trillium_logger::logger;
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;

struct PollCounter {
    reader: Cursor<String>,
    polls: usize,
    hash: u128,
}

impl AsyncRead for PollCounter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let result = Pin::new(&mut self.reader).poll_read(cx, buf);
        self.polls += 1;
        if let Poll::Ready(Ok(n)) = &result {
            for chunk in buf[0..*n].chunks(16) {
                for (offset, byte) in chunk.iter().enumerate() {
                    self.hash ^= u128::from(*byte) << offset;
                }
            }
        }
        result
    }
}

impl BodySource for PollCounter {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        Some(
            Headers::new()
                .with_inserted_header("poll-count", self.polls)
                .with_inserted_header("hash", format!("{:x}", self.hash)),
        )
    }
}

async fn handler_fn(mut conn: Conn) -> Conn {
    let request_body = conn.request_body_string().await.unwrap_or_default();
    let body = format!("trillium h3-example\n\n{conn:#?}\n\n===request body===\n\n{request_body}");
    conn.ok(Body::new_with_trailers(
        PollCounter {
            reader: Cursor::new(body),
            polls: 0,
            hash: 0,
        },
        None,
    ))
    .with_response_header(KnownHeaderName::Trailer, ["poll-count", "hash"])
}

fn cert_and_key() -> Option<(Vec<u8>, Vec<u8>)> {
    let host_path = std::env::var("CERT").ok()?;
    let key_path = std::env::var("KEY").ok()?;
    let cert_file = std::fs::read(host_path).ok()?;
    let key_file = std::fs::read(key_path).ok()?;
    Some((cert_file, key_file))
}

fn main() -> ExitCode {
    env_logger::init();

    let Some((cert, key)) = cert_and_key() else {
        eprintln!("CERT and KEY env vars should point at files");
        return ExitCode::FAILURE;
    };

    let Some(url) = std::env::var("URL")
        .ok()
        .as_deref()
        .map(Url::parse)
        .transpose()
        .ok()
        .flatten()
    else {
        eprintln!("URL env var expected");
        return ExitCode::FAILURE;
    };

    trillium_tokio::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert, &key))
        .with_quic(QuicConfig::from_single_cert(&cert, &key))
        .with_shared_state(url)
        .run((logger(), handler_fn));

    ExitCode::SUCCESS
}
