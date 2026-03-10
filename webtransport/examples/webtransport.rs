use futures_lite::{AsyncReadExt, AsyncWriteExt};
use std::fs;
use trillium_logger::logger;
use trillium_quinn::QuicConfig;
use trillium_rustls::RustlsAcceptor;
use trillium_static_compiled::static_compiled;
use trillium_webtransport::{InboundStream, WebTransport, WebTransportConnection};

async fn handle(wt: WebTransportConnection) {
    // Test open_uni: push a one-way welcome message.
    match wt.open_uni().await {
        Ok(mut stream) => {
            let _ = stream
                .write_all(b"Connected to trillium-webtransport!")
                .await;
            let _ = stream.close().await;
        }
        Err(e) => log::error!("open_uni failed: {e}"),
    }

    // Test open_bidi: server sends a greeting, half-closes its write side, then reads the
    // client's reply. Runs concurrently with the accept loop below.
    let server_greeting = async {
        match wt.open_bidi().await {
            Ok(mut stream) => {
                let ok = stream
                    .write_all(b"Hello from the server! Send me something back.")
                    .await
                    .is_ok();
                // Half-close the write side so the client sees EOF and knows the greeting is done.
                let _ = stream.close().await;
                if ok {
                    let mut buf = vec![0u8; 256];
                    match stream.read(&mut buf).await {
                        Ok(n) if n > 0 => {
                            log::info!(
                                "[bidi] client replied: {}",
                                String::from_utf8_lossy(&buf[..n])
                            );
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => log::error!("open_bidi failed: {e}"),
        }
    };

    // Datagram loop: dedicated low-latency path for unreliable messages.
    // Runs concurrently with stream acceptance so datagrams are never queued
    // behind slower stream I/O.
    let datagram_loop = async {
        // recv_datagram: echo back unchanged (client uses this for ping-pong RTT).
        while let Some(data) = wt.recv_datagram().await {
            let _ = wt.send_datagram(&data);
        }
    };

    // Stream accept loop: handles inbound bidi and uni streams.
    let stream_loop = async {
        while let Some(stream) = wt.accept_next_stream().await {
            match stream {
                // accept_bidi: echo message back as uppercase.
                InboundStream::Bidi(mut stream) => {
                    let mut buf = Vec::new();
                    if stream.read_to_end(&mut buf).await.is_ok_and(|n| n > 0) {
                        let response = String::from_utf8_lossy(&buf).to_uppercase();
                        if stream.write_all(response.as_bytes()).await.is_ok() {
                            let _ = stream.close().await;
                        }
                    }
                }

                // accept_uni: log the message, send an ack datagram.
                InboundStream::Uni(mut stream) => {
                    let mut buf = Vec::new();
                    if stream.read_to_end(&mut buf).await.is_ok_and(|n| n > 0) {
                        let msg = String::from_utf8_lossy(&buf);
                        log::info!("[uni] received: {msg}");
                        let ack = format!("ack:{msg}");
                        let _ = wt.send_datagram(ack.as_bytes());
                    }
                }
            }
        }
    };

    futures_lite::future::zip(
        server_greeting,
        futures_lite::future::zip(datagram_loop, stream_loop),
    )
    .await;
}

fn main() {
    env_logger::init();

    trillium_rustls::rustls::crypto::ring::default_provider()
        .install_default()
        .expect("installing default crypto provider");

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <cert.pem> <key.pem>", args[0]);
        std::process::exit(1);
    }

    let cert_pem = fs::read(&args[1]).expect("reading cert file");
    let key_pem = fs::read(&args[2]).expect("reading key file");

    trillium_smol::config()
        .with_acceptor(RustlsAcceptor::from_single_cert(&cert_pem, &key_pem))
        .with_quic(QuicConfig::from_single_cert(&cert_pem, &key_pem))
        .run((
            logger(),
            WebTransport::new(handle),
            static_compiled!("./examples/static").with_index_file("index.html"),
        ));
}
