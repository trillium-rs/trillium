use tracing::Span;
use trillium::{async_trait, Conn, Handler, Info, Status};

use crate::{formatters, response_time};

/// This is simple span handler
pub struct TracingHandler {
    span: Span,
}

impl TracingHandler {
    /// Creates new span
    pub fn new() -> Self {
        Self {
            span: Span::current(),
        }
    }
}

#[async_trait]
impl Handler for TracingHandler {
    async fn run(&self, conn: Conn) -> Conn {
        let _guard = self.span.enter();
        conn
    }
    async fn init(&mut self, info: &mut Info) {
        tracing::info!("Starting server");
        tracing::info!(server = ?info.server_description(), socker_addr = ?info.tcp_socket_addr().unwrap());
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        let path = conn.path();
        let method = conn.method();
        let response_len = conn.response_len().unwrap_or_default();
        let response_time = response_time(&conn, false).to_string();
        let ip = formatters::ip(&conn, false);
        let status = conn.status().unwrap_or(Status::NotFound);

        tracing::info!(method = ?method, response_length = ?response_len, path = ?path, ip = ?ip, resp_time = ?response_time, status = ?status);
        conn.inner_mut().after_send(move |_| {}); // this is unnecessary?
        conn
    }
}
