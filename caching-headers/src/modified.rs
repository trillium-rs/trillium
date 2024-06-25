use crate::CachingHeadersExt;
use trillium::{Conn, Handler, Status};

/// # A handler for the `Last-Modified` and `If-Modified-Since` header interaction.
///
/// This handler does not set a `Last-Modified` header on its own, but
/// relies on other handlers doing so.
#[derive(Debug, Clone, Copy, Default)]
pub struct Modified {
    _private: (),
}

impl Modified {
    /// constructs a new Modified handler
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Handler for Modified {
    async fn before_send(&self, conn: Conn) -> Conn {
        match (conn.if_modified_since(), conn.last_modified()) {
            (Some(if_modified_since), Some(last_modified))
                if last_modified <= if_modified_since =>
            {
                conn.with_status(Status::NotModified)
            }

            _ => conn,
        }
    }

    async fn run(&self, conn: Conn) -> Conn {
        conn
    }
}
