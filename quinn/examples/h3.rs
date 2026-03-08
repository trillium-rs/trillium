use std::fs;
use trillium::Conn;
use trillium_logger::logger;
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;

async fn handler_fn(conn: Conn) -> Conn {
    let body = format!("trillium h3-example\n\n{conn:#?}");
    conn.ok(body)
}

fn main() {
    env_logger::init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("installing default crypto provider");

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
