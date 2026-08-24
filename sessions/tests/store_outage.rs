use async_session::{Error, Result, Session, SessionStore, async_trait};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::SeqCst},
};
use trillium::{Conn, Handler, Info, Status};
use trillium_cookies::CookiesHandler;
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};
use trillium_testing::{TestServer, harness, test};

const SECRET: &str = "this is just for testing and you should not do this";

/// A store that is always unreachable, standing in for an outage of an external session backend.
#[derive(Debug, Clone, Copy)]
struct UnreachableStore;

#[async_trait]
impl SessionStore for UnreachableStore {
    async fn load_session(&self, _cookie_value: String) -> Result<Option<Session>> {
        Err(Error::msg("connection refused"))
    }

    async fn store_session(&self, _session: Session) -> Result<Option<String>> {
        Err(Error::msg("connection refused"))
    }

    async fn destroy_session(&self, _session: Session) -> Result {
        Err(Error::msg("connection refused"))
    }

    async fn clear_store(&self) -> Result {
        Err(Error::msg("connection refused"))
    }
}

/// A cookie signed with `SECRET`, so that the handler gets as far as asking the store for it.
async fn signed_cookie() -> String {
    let app = TestServer::new((
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), SECRET),
        count,
    ))
    .await;

    let response = app.get("/").await;
    let set_cookie = response.header("set-cookie").unwrap();
    let cookie = trillium_cookies::cookie::Cookie::parse_encoded(set_cookie).unwrap();
    format!("{}={}", cookie.name(), cookie.value())
}

async fn count(conn: Conn) -> Conn {
    let count: usize = conn.session().get("count").unwrap_or_default();
    conn.with_session("count", count + 1)
        .ok(format!("count: {count}"))
}

#[test(harness)]
async fn store_outage_halts_by_default() {
    let app = TestServer::new((
        CookiesHandler::new(),
        SessionHandler::new(UnreachableStore, SECRET),
        count,
    ))
    .await;

    app.get("/")
        .with_request_header("cookie", signed_cookie().await)
        .await
        .assert_status(Status::ServiceUnavailable);

    // no cookie means no load attempt, so a first-time visitor is unaffected by the read path
    app.get("/").await.assert_ok().assert_body("count: 0");
}

#[test(harness)]
async fn store_outage_can_continue_anonymously() {
    let app = TestServer::new((
        CookiesHandler::new(),
        SessionHandler::new(UnreachableStore, SECRET).with_store_error_handler(()),
        count,
    ))
    .await;

    app.get("/")
        .with_request_header("cookie", signed_cookie().await)
        .await
        .assert_ok()
        .assert_body("count: 0");
}

#[test(harness)]
async fn store_outage_handler_sees_the_error() {
    let app = TestServer::new((
        CookiesHandler::new(),
        SessionHandler::new(UnreachableStore, SECRET).with_store_error_handler(
            |conn: Conn| async move {
                let error = conn.session_store_error().unwrap().to_string();
                conn.with_status(Status::BadGateway).with_body(error).halt()
            },
        ),
        count,
    ))
    .await;

    app.get("/")
        .with_request_header("cookie", signed_cookie().await)
        .await
        .assert_status(Status::BadGateway)
        .assert_body("connection refused");
}

/// Records that the lifecycle hooks a wrapped handler expects were forwarded to it.
#[derive(Clone, Debug, Default)]
struct Recorder {
    init: Arc<AtomicBool>,
    before_send: Arc<AtomicBool>,
    halt: bool,
}

impl Handler for Recorder {
    async fn run(&self, conn: Conn) -> Conn {
        if self.halt { conn.halt() } else { conn }
    }

    async fn init(&mut self, _info: &mut Info) {
        self.init.store(true, SeqCst);
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.before_send.store(true, SeqCst);
        conn
    }
}

async fn run_with(recorder: Recorder) {
    let app = TestServer::new((
        CookiesHandler::new(),
        SessionHandler::new(UnreachableStore, SECRET).with_store_error_handler(recorder),
        count,
    ))
    .await;

    app.get("/")
        .with_request_header("cookie", signed_cookie().await)
        .await;
}

#[test(harness)]
async fn store_error_handler_is_initialized() {
    let recorder = Recorder::default();
    run_with(recorder.clone()).await;
    assert!(recorder.init.load(SeqCst));
}

#[test(harness)]
async fn store_error_handler_before_send_runs_when_it_halts() {
    let recorder = Recorder {
        halt: true,
        ..Recorder::default()
    };
    run_with(recorder.clone()).await;
    assert!(recorder.before_send.load(SeqCst));
}

#[test(harness)]
async fn store_error_handler_before_send_runs_when_it_does_not_halt() {
    let recorder = Recorder::default();
    run_with(recorder.clone()).await;
    assert!(recorder.before_send.load(SeqCst));
}
