//! Bring up a trillium server for conformance testing.
//!
//! Each runtime adapter gets its own `run_on_<runtime>` fn — not abstractable into a
//! generic because each runtime's `block_on` and `spawn` machinery is distinct. Each one:
//!
//! 1. Spins up its runtime on a dedicated thread.
//! 2. Binds port 0 on localhost, with the chosen TLS acceptor.
//! 3. Announces the bound port via a sync channel so the main (sync) thread can point h2spec at it.
//! 4. Blocks the thread until the caller signals shutdown via the returned `ServerHandle`.
//!
//! The handler is a stateless "hello" echo that also reads the request body, so tests that
//! depend on the server attempting to consume body data (e.g. §8.1.2.6 content-length
//! mismatch) exercise the full recv path.

use crate::{Runtime, Tls};
use std::{
    net::SocketAddr,
    sync::mpsc,
    thread::{self, JoinHandle},
};
use trillium::Conn;
use trillium_rustls::RustlsAcceptor;
use trillium_tokio::Swansong;

/// Hello handler: read the body (so content-length validation has something to check
/// against) then respond 200 with a fixed body.
async fn hello(mut conn: Conn) -> Conn {
    let _ = conn.request_body_string().await;
    conn.ok("hello from trillium-conformance")
}

/// Handle returned from `start_server`. Dropping it (or calling `shut_down`) signals the
/// server thread to exit and waits for it.
pub struct ServerHandle {
    pub addr: SocketAddr,
    pub shutdown: Box<dyn FnOnce() + Send>,
    pub join: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn shut_down(mut self) {
        (self.shutdown)();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Build the TLS acceptor for a given mode. `None` is cleartext; `Rustls` generates a
/// fresh self-signed cert for `localhost`.
pub fn rustls_acceptor() -> RustlsAcceptor {
    let rcgen::CertifiedKey {
        cert, signing_key, ..
    } = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("rcgen self-signed cert");
    RustlsAcceptor::from_single_cert(
        cert.pem().as_bytes(),
        signing_key.serialize_pem().as_bytes(),
    )
}

/// Start a trillium server on the chosen runtime + TLS, block until it's bound, return a
/// handle carrying its address and a shutdown hook.
pub fn start(runtime: Runtime, tls: Tls) -> anyhow::Result<ServerHandle> {
    match (runtime, tls) {
        (Runtime::Tokio, Tls::None) => start_tokio_cleartext(),
        (Runtime::Tokio, Tls::Rustls) => start_tokio_rustls(),
        (Runtime::Smol, Tls::None) => start_smol_cleartext(),
        (Runtime::Smol, Tls::Rustls) => start_smol_rustls(),
        (Runtime::AsyncStd, Tls::None) => start_async_std_cleartext(),
        (Runtime::AsyncStd, Tls::Rustls) => start_async_std_rustls(),
    }
}

// Shared channel plumbing so each runtime's bringup fn reads the same. The inner async
// block is distinct per runtime because config() / spawn() are runtime-specific.
struct Wiring {
    addr_tx: mpsc::SyncSender<SocketAddr>,
    swansong: Swansong,
}

fn wiring() -> (Wiring, mpsc::Receiver<SocketAddr>, Swansong) {
    let (addr_tx, addr_rx) = mpsc::sync_channel::<SocketAddr>(1);
    let swansong = Swansong::new();
    (
        Wiring {
            addr_tx,
            swansong: swansong.clone(),
        },
        addr_rx,
        swansong,
    )
}

fn join_with_shutdown(
    addr_rx: mpsc::Receiver<SocketAddr>,
    swansong: Swansong,
    join: JoinHandle<()>,
) -> anyhow::Result<ServerHandle> {
    let addr = addr_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("server thread exited before announcing its bound address"))?;
    let swansong_for_shutdown = swansong;
    Ok(ServerHandle {
        addr,
        shutdown: Box::new(move || {
            swansong_for_shutdown.shut_down();
        }),
        join: Some(join),
    })
}

fn start_tokio_cleartext() -> anyhow::Result<ServerHandle> {
    let (w, addr_rx, swansong) = wiring();
    let join = thread::spawn(move || {
        let rt = trillium_tokio::tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async move {
            let handle = trillium_tokio::config()
                .with_host("127.0.0.1")
                .with_port(0)
                .with_swansong(w.swansong)
                .without_signals()
                .spawn(hello);
            let info = handle.info().await;
            if let Some(addr) = info.tcp_socket_addr() {
                let _ = w.addr_tx.send(*addr);
            }
            handle.await;
        });
    });
    join_with_shutdown(addr_rx, swansong, join)
}

fn start_tokio_rustls() -> anyhow::Result<ServerHandle> {
    let acceptor = rustls_acceptor();
    let (w, addr_rx, swansong) = wiring();
    let join = thread::spawn(move || {
        let rt = trillium_tokio::tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async move {
            let handle = trillium_tokio::config()
                .with_host("127.0.0.1")
                .with_port(0)
                .with_swansong(w.swansong)
                .without_signals()
                .with_acceptor(acceptor)
                .spawn(hello);
            let info = handle.info().await;
            if let Some(addr) = info.tcp_socket_addr() {
                let _ = w.addr_tx.send(*addr);
            }
            handle.await;
        });
    });
    join_with_shutdown(addr_rx, swansong, join)
}

fn start_smol_cleartext() -> anyhow::Result<ServerHandle> {
    let (w, addr_rx, swansong) = wiring();
    let join = thread::spawn(move || {
        trillium_smol::async_global_executor::block_on(async move {
            let handle = trillium_smol::config()
                .with_host("127.0.0.1")
                .with_port(0)
                .with_swansong(w.swansong)
                .without_signals()
                .spawn(hello);
            let info = handle.info().await;
            if let Some(addr) = info.tcp_socket_addr() {
                let _ = w.addr_tx.send(*addr);
            }
            handle.await;
        });
    });
    join_with_shutdown(addr_rx, swansong, join)
}

fn start_smol_rustls() -> anyhow::Result<ServerHandle> {
    let acceptor = rustls_acceptor();
    let (w, addr_rx, swansong) = wiring();
    let join = thread::spawn(move || {
        trillium_smol::async_global_executor::block_on(async move {
            let handle = trillium_smol::config()
                .with_host("127.0.0.1")
                .with_port(0)
                .with_swansong(w.swansong)
                .without_signals()
                .with_acceptor(acceptor)
                .spawn(hello);
            let info = handle.info().await;
            if let Some(addr) = info.tcp_socket_addr() {
                let _ = w.addr_tx.send(*addr);
            }
            handle.await;
        });
    });
    join_with_shutdown(addr_rx, swansong, join)
}

fn start_async_std_cleartext() -> anyhow::Result<ServerHandle> {
    let (w, addr_rx, swansong) = wiring();
    let join = thread::spawn(move || {
        trillium_async_std::async_std::task::block_on(async move {
            let handle = trillium_async_std::config()
                .with_host("127.0.0.1")
                .with_port(0)
                .with_swansong(w.swansong)
                .without_signals()
                .spawn(hello);
            let info = handle.info().await;
            if let Some(addr) = info.tcp_socket_addr() {
                let _ = w.addr_tx.send(*addr);
            }
            handle.await;
        });
    });
    join_with_shutdown(addr_rx, swansong, join)
}

fn start_async_std_rustls() -> anyhow::Result<ServerHandle> {
    let acceptor = rustls_acceptor();
    let (w, addr_rx, swansong) = wiring();
    let join = thread::spawn(move || {
        trillium_async_std::async_std::task::block_on(async move {
            let handle = trillium_async_std::config()
                .with_host("127.0.0.1")
                .with_port(0)
                .with_swansong(w.swansong)
                .without_signals()
                .with_acceptor(acceptor)
                .spawn(hello);
            let info = handle.info().await;
            if let Some(addr) = info.tcp_socket_addr() {
                let _ = w.addr_tx.send(*addr);
            }
            handle.await;
        });
    });
    join_with_shutdown(addr_rx, swansong, join)
}
