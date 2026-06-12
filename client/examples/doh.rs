//! DNS-over-HTTPS (DoH) client example.
//!
//! Routes *all* of the client's DNS — A/AAAA records and SVCB/HTTPS records
//! alike — through a chosen DoH resolver instead of the system resolver, then
//! fetches real domains over the addresses it returns. The lookups themselves
//! ride the client's own connection pool to the resolver (so DoH-over-HTTP/2),
//! and never touch the system resolver once `with_doh` is set.
//!
//! When a domain publishes an HTTPS record advertising `alpn=h3`, the *first*
//! request goes straight over HTTP/3 — no Alt-Svc round-trip needed, because the
//! SVCB record told us h3 was available before we connected.
//!
//! Usage:
//!   cargo run -p trillium-client --example doh --features hickory
//!   cargo run -p trillium-client --example doh --features hickory -- 8.8.8.8
//!   cargo run -p trillium-client --example doh --features hickory -- 1.1.1.1 https://example.com/
//!
//! With no arguments it runs against both Cloudflare (`1.1.1.1`) and Google
//! (`8.8.8.8`). The first argument, if given, overrides the resolver (a bare IP
//! or a full `https://.../dns-query` URL); any remaining arguments are URLs to
//! fetch. Set `DOH_HTTP3=1` to pin the resolver connection itself to HTTP/3
//! (DoH-over-h3). Requires the `hickory` feature.
use trillium_client::Client;
use trillium_quinn::ClientQuicConfig;
use trillium_rustls::RustlsConfig;
use trillium_tokio::{ClientConfig, TokioRuntime};

fn main() {
    env_logger::init();

    let mut args = std::env::args().skip(1);

    let resolvers: Vec<String> = match args.next() {
        Some(resolver) => vec![resolver],
        // Cloudflare and Google, the two ubiquitous public DoH resolvers.
        None => vec!["1.1.1.1".into(), "8.8.8.8".into()],
    };

    let urls: Vec<String> = {
        let rest: Vec<String> = args.collect();
        if rest.is_empty() {
            // cloudflare.com and blog.cloudflare.com publish HTTPS records with
            // `alpn=h3` (expect HTTP/3); example.com does not (expect h1/h2).
            [
                "https://www.cloudflare.com/",
                "https://blog.cloudflare.com/",
                "https://example.com/",
            ]
            .into_iter()
            .map(String::from)
            .collect()
        } else {
            rest
        }
    };

    TokioRuntime::default().block_on(async move {
        for resolver in &resolvers {
            println!("\n=== routing all DNS through {resolver} ===");

            let client = Client::new_with_quic(
                RustlsConfig::<ClientConfig>::default(),
                ClientQuicConfig::with_webpki_roots(),
            );
            let client = if std::env::var("DOH_HTTP3").is_ok() {
                client.with_doh3(resolver)
            } else {
                client.with_doh(resolver)
            };

            for url in &urls {
                match client.get(url.as_str()).await {
                    Ok(conn) => {
                        let version = conn.http_version();
                        let status = conn.status().map(|s| s.to_string()).unwrap_or_default();
                        println!("  {status} via {version:?}  {url}");
                    }
                    Err(e) => println!("  error — {e}  {url}"),
                }
            }
        }
    });
}
