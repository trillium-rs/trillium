//! # Trillium handler for HEAD requests
//!
//! This simple handler rewrites HEAD requests to be GET requests, and
//! then before sending the response, removes the outbound body but sets a
//! content length header. Any handlers subsequent to this one see a GET
//! request.
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

use trillium::{Conn, Handler, KnownHeaderName::ContentLength, Method, conn_unwrap};

/// Trillium handler for HEAD requests
///
/// See crate-level docs for an explanation
#[derive(Default, Clone, Copy, Debug)]
pub struct Head {
    _my_private_things: (),
}

impl Head {
    /// constructs a new Head handler
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone, Copy, Debug)]
struct RequestWasHead;

impl Handler for Head {
    async fn run(&self, mut conn: Conn) -> Conn {
        if conn.method() == Method::Head {
            conn.inner_mut().set_method(Method::Get);
            conn.insert_state(RequestWasHead);
        }

        conn
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        conn_unwrap!(conn.state::<RequestWasHead>(), conn);
        conn.inner_mut().set_method(Method::Head);
        let len = conn_unwrap!(
            conn.inner_mut()
                .take_response_body()
                .and_then(|body| body.len()),
            conn
        );
        conn.with_response_header(ContentLength, len.to_string())
    }
}

/// Alias for [`Head::new`]
pub fn head() -> Head {
    Head::new()
}
