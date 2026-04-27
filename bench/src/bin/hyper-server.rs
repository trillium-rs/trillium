//! Hyper baseline bench server. Same routes, same TLS setup as the trillium variant.
//!
//! No router crate — match-on-path. The point is the lowest-overhead reference.

use bytes::Bytes;
use clap::Parser;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rustls::ServerConfig;
use rustls::pki_types::PrivateKeyDer;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[derive(Parser, Debug)]
#[command(about = "hyper h2 baseline bench server")]
struct Args {
    #[arg(long, default_value_t = 8443)]
    port: u16,
    #[arg(
        long,
        default_value = "/home/ubuntu/trillium-h2-bench.tanuki-sunfish.ts.net.crt"
    )]
    cert: PathBuf,
    #[arg(
        long,
        default_value = "/home/ubuntu/trillium-h2-bench.tanuki-sunfish.ts.net.key"
    )]
    key: PathBuf,
}

struct Bodies {
    one_k: Bytes,
    sixteen_k: Bytes,
    sixty_four_k: Bytes,
    one_m: Bytes,
    ten_m: Bytes,
}

fn bodies() -> &'static Bodies {
    static B: OnceLock<Bodies> = OnceLock::new();
    B.get_or_init(|| {
        let make = |n: usize| Bytes::from(vec![b'x'; n]);
        Bodies {
            one_k: make(1024),
            sixteen_k: make(16 * 1024),
            sixty_four_k: make(64 * 1024),
            one_m: make(1024 * 1024),
            ten_m: make(10 * 1024 * 1024),
        }
    })
}

fn body_for(size: &str) -> Option<Bytes> {
    let b = bodies();
    Some(match size {
        "1k" => b.one_k.clone(),
        "16k" => b.sixteen_k.clone(),
        "64k" => b.sixty_four_k.clone(),
        "1m" => b.one_m.clone(),
        "10m" => b.ten_m.clone(),
        _ => return None,
    })
}

async fn handle(
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let path = req.uri().path();
    let method = req.method();

    match (method, path) {
        (&Method::GET, "/tiny") => Ok(Response::new(Full::new(Bytes::from_static(b"ok")))),
        (&Method::GET, "/small") => Ok(Response::new(Full::new(bodies().one_k.clone()))),
        (&Method::GET, p) if p.starts_with("/large/") => {
            let size = &p["/large/".len()..];
            match body_for(size) {
                Some(body) => Ok(Response::new(Full::new(body))),
                None => Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::new()))
                    .unwrap()),
            }
        }
        (&Method::POST, "/echo") => {
            let body = req.collect().await.map(|c| c.to_bytes()).unwrap_or_default();
            Ok(Response::new(Full::new(body)))
        }
        (&Method::POST, "/recv") => {
            let body = req.collect().await.map(|c| c.to_bytes()).unwrap_or_default();
            Ok(Response::new(Full::new(Bytes::from(body.len().to_string()))))
        }
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::new()))
            .unwrap()),
    }
}

fn load_tls(cert_path: &PathBuf, key_path: &PathBuf) -> ServerConfig {
    let cert_file = std::fs::File::open(cert_path).expect("open cert");
    let key_file = std::fs::File::open(key_path).expect("open key");
    let certs = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    let mut keys = BufReader::new(key_file);
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut keys)
        .expect("read key")
        .expect("at least one private key");

    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("build server config");
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    ServerConfig::from(cfg)
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let args = Args::parse();
    bodies();

    let tls_cfg = Arc::new(load_tls(&args.cert, &args.key));
    let acceptor = TlsAcceptor::from(tls_cfg);
    let addr: SocketAddr = ([0, 0, 0, 0], args.port).into();
    let listener = TcpListener::bind(addr).await.expect("bind");

    log::info!("hyper bench listening on {}", addr);

    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                log::warn!("accept error: {e}");
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let tls = match acceptor.accept(stream).await {
                Ok(t) => t,
                Err(_) => return,
            };
            let io = TokioIo::new(tls);
            let mut builder = auto::Builder::new(TokioExecutor::new());
            builder.http2().max_concurrent_streams(100);
            let _ = builder.serve_connection(io, service_fn(handle)).await;
        });
    }
}
