//! Implementation-neutral golden-file corpus for the client's HTTP/1.x response parser.
//!
//! Each `tests/corpus_h1/*.response` fixture is the raw bytes a server sends back, using
//! `\r` / `\n` / `\t` / `\0` escaping (a superset of the trillium-http request corpus): literal
//! line breaks in the file are stripped for readability, and only the escape sequences become real
//! bytes. An optional first line beginning with `>>> ` scripts the request as `>>> METHOD
//! [VERSION]` (e.g. `>>> HEAD HTTP/1.1`); absent, the request is `GET / HTTP/1.1`. Only the method
//! and version affect response framing, so that's all the directive carries.
//!
//! The harness drives a real client [`Conn`] against the bytes over a `TestTransport`, then writes
//! a golden capturing the observable outcome:
//!
//! - `*.parsed` — an accepted response: status, response headers, decoded body, trailers, and the
//!   keep-alive/reuse decision.
//! - `*.error` — a rejected response: the error the client surfaced.
//!
//! A fixed [`SENTINEL`] is appended after every fixture's bytes (then the transport's write half is
//! shut). Correct framing leaves it unconsumed, so it never appears in the decoded body; an
//! over-read or a read-to-close body swallows it, making the boundary error visible in the golden.
//! This is the smuggling-relevant signal — where the client draws the body boundary — and it needs
//! no knowledge of the client internals to read.
//!
//! Regenerate goldens with `CORPUS_H1_WRITE=1`; filter cases with `CORPUS_H1_FILTER=substr`.

use async_channel::Sender;
use pretty_assertions::assert_str_eq;
use std::{
    env,
    future::{Future, IntoFuture},
    io,
    net::{Shutdown, SocketAddr},
    path::PathBuf,
    str::FromStr,
};
use test_harness::test;
use trillium_client::{Client, Conn, Error, Method, Version};
use trillium_server_common::{Connector, Url};
use trillium_testing::{RuntimeTrait, TestTransport, harness};

/// Appended after every fixture's response bytes. A correctly-framed delimited body stops before
/// it; a read-to-close or over-reading body consumes it. Its presence (or absence) in the golden
/// `===body===` section reveals where the client drew the body boundary.
const SENTINEL: &str = "[PIPELINED-SENTINEL]";

fn unescape(s: &str) -> String {
    s.replace(['\r', '\n'], "")
        .replace("\\r", "\r")
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\0", "\0")
}

fn escape(s: &str) -> String {
    s.replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\n', "\\n\n")
}

/// Parse a `>>> METHOD [VERSION]` directive line into the request to issue.
fn parse_directive(line: &str) -> (Method, Version) {
    let mut tokens = line.trim_start_matches('>').split_whitespace();
    let method = tokens
        .next()
        .and_then(|m| Method::from_str(m).ok())
        .unwrap_or(Method::Get);
    let version = tokens
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(Version::Http1_1);
    (method, version)
}

#[test(harness)]
async fn corpus_h1() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus_h1");
    let filter = env::var("CORPUS_H1_FILTER").unwrap_or_default();

    let fixtures = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|f| {
            let path = f.unwrap().path();
            (path.extension().and_then(|x| x.to_str()) == Some("response")).then_some(path)
        })
        .filter(|f| f.to_str().unwrap().contains(&filter));

    for file in fixtures {
        let raw = std::fs::read_to_string(&file)
            .unwrap_or_else(|_| panic!("could not read {}", file.display()));

        let (method, version, body_src) = if raw.starts_with(">>>") {
            let (first, rest) = raw.split_once('\n').unwrap_or((&raw, ""));
            let (method, version) = parse_directive(first);
            (method, version, rest.to_string())
        } else {
            (Method::Get, Version::Http1_1, raw)
        };

        let response = unescape(&body_src);

        let (transport, conn_fut) = test_conn(method, version).await;
        transport.write_all(format!("{response}{SENTINEL}"));
        transport.shutdown(Shutdown::Write);

        let (golden, extension) = match conn_fut.await {
            Ok(mut conn) => {
                let body = conn.response_body().read_bytes().await;
                let body = match body {
                    Ok(bytes) => escape(&String::from_utf8_lossy(&bytes)),
                    Err(e) => format!("<body read error: {e}>"),
                };
                let trailers = conn
                    .response_trailers()
                    .map(|t| format!("===trailers===\n{}\n", t))
                    .unwrap_or_default();
                let status = conn
                    .status()
                    .map_or_else(|| "<none>".to_string(), |s| s.to_string());
                let version = conn.http_version();
                let headers = conn.response_headers();
                let keep_alive = conn.is_keep_alive();
                // Built section-by-section rather than as one literal: `format_strings = true`
                // rewraps long string literals and mangles `\n` escapes near the wrap point.
                let mut golden = String::new();
                golden += &format!("===status===\n{status}\n\n");
                golden += &format!("===version===\n{version}\n\n");
                golden += &format!("===headers===\n{headers}\n");
                golden += &format!("===keep-alive===\n{keep_alive}\n\n");
                golden += &format!("===body===\n{body}\n");
                golden += &trailers;
                (golden, "parsed")
            }
            Err(e) => (format!("{e}\n"), "error"),
        };

        let golden_file = file.with_extension(extension);

        if option_env!("CORPUS_H1_WRITE").is_some() {
            std::fs::write(&golden_file, escape(&golden)).unwrap();
        } else {
            let expected = unescape(
                &std::fs::read_to_string(&golden_file)
                    .unwrap_or_else(|_| panic!("missing golden {}", golden_file.display())),
            );
            assert_str_eq!(expected, golden, "\n\n{file:?}");
        }
    }
}

struct TestConnector<R>(Sender<TestTransport>, R);

impl<R: RuntimeTrait> Connector for TestConnector<R> {
    type Runtime = R;
    type Transport = TestTransport;
    type Udp = ();

    async fn connect(&self, _url: &Url) -> io::Result<Self::Transport> {
        let (server, client) = TestTransport::new();
        let _ = self.0.send(server).await;
        Ok(client)
    }

    fn runtime(&self) -> Self::Runtime {
        self.1.clone()
    }

    async fn resolve(&self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(vec![])
    }
}

async fn test_conn(
    method: Method,
    version: Version,
) -> (TestTransport, impl Future<Output = Result<Conn, Error>>) {
    let (sender, receiver) = async_channel::unbounded();
    let client = Client::new(TestConnector(sender, trillium_testing::runtime()));
    let runtime = client.connector().runtime();
    let conn = client
        .build_conn(method, "http://example.com/")
        .with_http_version(version);
    let conn_fut = runtime.spawn(conn.into_future()).into_future();
    let transport = receiver.recv().await.unwrap();
    (transport, async move { conn_fut.await.unwrap() })
}
