/// HTTP/3 client example demonstrating Alt-Svc-based protocol upgrade.
///
/// Usage:
///   cargo run -p trillium-client --example h3_client -- https://cloudflare.com/
///
/// What to expect:
///   - Request 1: HTTP/1.1 (no Alt-Svc cached yet)
///   - Request 2+: HTTP/3 if the server advertised `Alt-Svc: h3=...` in the first response
///
/// Any HTTPS server that supports HTTP/3 will work. cloudflare.com and blog.cloudflare.com
/// are reliable test targets.
use trillium_client::Client;
use trillium_quinn::ClientQuicConfig;
use trillium_rustls::RustlsConfig;
use trillium_tokio::{ClientConfig, TokioRuntime};

fn main() {
    env_logger::init();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.cloudflare.com/".into());

    TokioRuntime::default().block_on(async move {
        let client = Client::new_with_quic(
            RustlsConfig::<ClientConfig>::default(),
            ClientQuicConfig::with_webpki_roots(),
        );

        for i in 1..=4 {
            match client.get(url.as_str()).await {
                Ok(conn) => {
                    let version = conn.http_version();
                    let status = conn.status().map(|s| s.to_string()).unwrap_or_default();
                    println!("request {i}: {status} via {version:?}");
                }
                Err(e) => {
                    println!("request {i}: error — {e}");
                }
            }
        }
    });
}
