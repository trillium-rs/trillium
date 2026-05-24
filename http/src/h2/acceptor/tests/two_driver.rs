//! Two-driver wire tests: a real client-role driver and a server-role driver wired over a
//! shared [`TestTransport`] pair and stepped in controlled order.
//!
//! The single-driver [`DriverFixture`][super::fixture::DriverFixture] models a scripted,
//! always-cooperative peer — it can't express the emergent cross-connection behavior of a
//! graceful-shutdown drain. Here both real drivers run, so we can reproduce that interleaving
//! deterministically (TestTransport delivers bytes synchronously, so stepping the two drivers in a
//! fixed order is fully repeatable).

use super::fixture::noop_waker;
use crate::{
    Conn, Headers, HttpContext, Method,
    h2::{
        H2Driver, H2Transport,
        acceptor::types::{CloseOutcome, DriverState},
        connection::H2Connection,
        role::Role,
    },
    headers::hpack::PseudoHeaders,
};
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use trillium_testing::TestTransport;

/// A client driver and server driver sharing one transport pair, plus the cloned transport
/// handles so the harness can observe wire progress and detect quiescence.
struct TwoDrivers {
    client: H2Driver<TestTransport>,
    client_conn: Arc<H2Connection>,
    client_finished: bool,
    server: H2Driver<TestTransport>,
    server_finished: bool,
}

impl TwoDrivers {
    fn new() -> Self {
        let (client_t, server_t) = TestTransport::new();
        let client_conn = H2Connection::new(Arc::new(HttpContext::new()));
        let server_conn = H2Connection::new(Arc::new(HttpContext::new()));
        let client = H2Driver::new(client_conn.clone(), client_t, Role::Client);
        let server = H2Driver::new(server_conn.clone(), server_t, Role::Server);
        Self {
            client,
            client_conn,
            client_finished: false,
            server,
            server_finished: false,
        }
    }

    /// Poll both drivers a fixed, generous number of rounds, collecting any server-yielded
    /// `Conn`s and latching each driver's terminal `Ready(None)`. Determinism + synchronous
    /// byte delivery mean a single-stream lifecycle fully settles well inside the bound; a
    /// driver still returning `Pending` at the end is genuinely stuck.
    fn pump(&mut self) -> Vec<Conn<H2Transport>> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut conns = Vec::new();
        for _ in 0..100 {
            if !self.server_finished {
                match self.server.drive(&mut cx) {
                    Poll::Ready(Some(Ok(conn))) => conns.push(conn),
                    Poll::Ready(Some(Err(_))) => {}
                    Poll::Ready(None) => self.server_finished = true,
                    Poll::Pending => {}
                }
            }
            if !self.client_finished {
                match self.client.drive(&mut cx) {
                    Poll::Ready(Some(Ok(_)) | Some(Err(_))) => {}
                    Poll::Ready(None) => self.client_finished = true,
                    Poll::Pending => {}
                }
            }
        }
        conns
    }
}

fn client_get_pseudos() -> PseudoHeaders<'static> {
    PseudoHeaders::default()
        .with_method(Method::Get)
        .with_path("/")
        .with_scheme("http")
        .with_authority("test")
}

/// Trace-faithful reproduction: client opens a request, server yields the `Conn`, then both
/// peers abandon the stream at once (each drops its transport → `RST_STREAM(Cancel)`) while
/// the server begins a graceful shutdown. Both drivers must finish their close-out; a driver
/// still `Pending` after the pump is the deadlock.
#[test]
fn double_reset_then_graceful_shutdown_drains() {
    let mut d = TwoDrivers::new();
    // Handshake: pump until both reach Running.
    d.pump();
    assert_eq!(d.client.state, DriverState::Running, "client handshake");
    assert_eq!(d.server.state, DriverState::Running, "server handshake");

    // Client sends a body-less GET; server yields the Conn.
    let (_id, submit, client_transport) = d
        .client_conn
        .open_stream(client_get_pseudos(), Headers::new(), None)
        .expect("open_stream on a running client connection");
    let mut conns = d.pump();
    let server_conn = conns
        .pop()
        .expect("server should yield a Conn for the request");
    assert!(conns.is_empty(), "exactly one request stream expected");

    // Both ends give up at once, and the server begins graceful shutdown.
    drop(submit);
    drop(client_transport);
    drop(server_conn);
    d.server.begin_close(CloseOutcome::Graceful);

    d.pump();

    assert!(
        d.server_finished,
        "server's graceful shutdown must drain and finish (state={:?})",
        d.server.state,
    );
    assert!(
        d.client_finished,
        "client driver must finish once the stream is reset and the connection closes (state={:?})",
        d.client.state,
    );
}

/// Variant: the client keeps its request transport (still awaiting a response) while the
/// server abandons the stream and shuts down. The client must learn the stream is gone (via
/// the server's RST) and finish — it can't self-heal by framing its own reset here, so this
/// isolates whether the server's RST actually reaches the client across the shutdown.
#[test]
fn server_abandon_and_shutdown_with_client_awaiting_response_drains() {
    let mut d = TwoDrivers::new();
    d.pump();

    // Hold the request handles so the client stream isn't torn down by a local drop — the
    // gate must clear from the server's RST, not the client's own teardown.
    let (_id, _submit, _client_transport) = d
        .client_conn
        .open_stream(client_get_pseudos(), Headers::new(), None)
        .expect("open_stream");
    let mut conns = d.pump();
    let server_conn = conns.pop().expect("server should yield a Conn");

    // Server gives up + shuts down; client holds its stream, still awaiting the response.
    drop(server_conn);
    d.server.begin_close(CloseOutcome::Graceful);

    d.pump();

    assert!(
        d.server_finished,
        "server must finish (state={:?})",
        d.server.state,
    );
    assert!(
        d.client_finished,
        "client awaiting a response must finish once the server resets + closes (state={:?})",
        d.client.state,
    );
}
