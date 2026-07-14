use crate::CachingHeadersExt;
use trillium::{Conn, Handler, KnownHeaderName, Status};

/// # A handler for the `Last-Modified` and `If-Modified-Since` header interaction.
///
/// This handler does not set a `Last-Modified` header on its own, but
/// relies on other handlers doing so.
///
/// ## Precedence: `If-None-Match` wins
///
/// `If-Modified-Since` is evaluated only when the request carries no
/// `If-None-Match`. Per [RFC 9110 §13.1.3][rfc]:
///
/// > A recipient MUST ignore If-Modified-Since if the request contains an
/// > If-None-Match header field; the condition in If-None-Match is considered to
/// > be a more accurate replacement for the condition in If-Modified-Since, and
/// > the two are only combined for the sake of interoperating with older
/// > intermediaries that might not implement If-None-Match.
///
/// This matters most in the case it is easiest to overlook: when the entity tag
/// did *not* match. Honoring both conditions would let a coarse timestamp
/// comparison override an [`Etag`] that had already determined the representation
/// changed, answering `304` for a body that is genuinely new. Browsers routinely
/// replay both headers, so any handler whose `Last-Modified` is coarser than its
/// entity tag — a rendered response whose inputs are versioned rather than
/// timestamped, say — would otherwise serve stale content.
///
/// [rfc]: https://www.rfc-editor.org/rfc/rfc9110#section-13.1.3
/// [`Etag`]: crate::Etag
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
        // RFC 9110 §13.1.3: an If-None-Match in the request wholly replaces
        // If-Modified-Since — including when it did not match, which is exactly
        // the case where evaluating both would go wrong.
        //
        // Presence of the header field, deliberately, not a successfully parsed
        // entity tag: `*` and malformed tags do not parse (`if_none_match()`
        // yields `None` for both), but the field is still there, and the spec
        // conditions on the field. Falling back to a timestamp comparison
        // because we could not read the tag would resurrect the very bug this
        // guards against.
        if conn
            .request_headers()
            .has_header(KnownHeaderName::IfNoneMatch)
        {
            return conn;
        }

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
