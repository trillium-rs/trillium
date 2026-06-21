#![allow(dead_code)]
//! Shared helpers for the server-side logger integration tests.

use std::sync::Arc;
use trillium::{BoxedHandler, Conn, KnownHeaderName::ContentType, Status};
use trillium_logger::{ColorMode, LogFormatter, Targetable, logger};
use trillium_testing::TestServer;

/// A [`Targetable`] that pushes every written line onto an unbounded channel, so tests can await
/// the next line without racing the server's `after_send` callback.
#[derive(Clone, Debug)]
pub struct CollectTarget(
    Arc<(
        async_channel::Sender<String>,
        async_channel::Receiver<String>,
    )>,
);

impl Default for CollectTarget {
    fn default() -> Self {
        Self(Arc::new(async_channel::unbounded()))
    }
}

impl Targetable for CollectTarget {
    fn write(&self, data: String) {
        self.0.0.send_blocking(data).unwrap();
    }
}

impl CollectTarget {
    pub async fn next(&self) -> String {
        self.0.1.recv().await.unwrap()
    }

    pub fn try_next(&self) -> Option<String> {
        self.0.1.try_recv().ok()
    }
}

/// The downstream handler under test: a teapot with a known body and content-type, so body-length
/// and response-header formatters have something deterministic to render.
pub async fn teapot(conn: Conn) -> Conn {
    conn.with_status(Status::ImATeapot)
        .with_response_header(ContentType, "text/plain")
        .with_body("ok")
}

/// Build a [`TestServer`] whose logger uses `formatter`, paired with the target it writes to.
pub async fn server(
    formatter: impl LogFormatter,
    color: ColorMode,
) -> (TestServer<BoxedHandler>, CollectTarget) {
    let target = CollectTarget::default();
    let logger = logger()
        .with_formatter(formatter)
        .with_target(target.clone())
        .with_color_mode(color)
        .without_init_message();
    let handler = BoxedHandler::new((logger, teapot));
    (TestServer::new(handler).await, target)
}
