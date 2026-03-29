use futures_lite::{AsyncRead, io::Cursor};
use std::{
    fs,
    io::Result,
    pin::Pin,
    task::{Context, Poll},
};
use trillium::{Body, BodySource, Conn, Headers, KnownHeaderName};
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
                for (offset, byte) in chunk.into_iter().enumerate() {
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

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <cert.pem> <key.pem>", args[0]);
        std::process::exit(1);
    }

    let cert_pem = fs::read(&args[1]).expect("reading cert file");
    let key_pem = fs::read(&args[2]).expect("reading key file");

    trillium_tokio::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert_pem, &key_pem))
        .with_quic(QuicConfig::from_single_cert(&cert_pem, &key_pem))
        .run((logger(), handler_fn));
}
