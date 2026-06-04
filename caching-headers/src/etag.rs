use crate::CachingHeadersExt;
use etag::EntityTag;
use trillium::{Conn, Handler, KnownHeaderName, Status};

/// # Etag and If-None-Match header handler
///
/// Trillium handler that provides an outbound [`etag
/// header`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/ETag)
/// after other handlers have been run, and if the request includes an
/// [`if-none-match`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/If-None-Match)
/// header, compares these values and sends a
/// [`304 not modified`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/304) status,
/// omitting the response body.
///
/// ## Streamed bodies
///
/// Note that this handler does not currently provide an etag trailer for
/// streamed bodies, but may do so in the future.
///
/// ## Strong vs weak comparison
///
/// Etags can be compared using a strong method or a weak
/// method. By default, this handler allows weak comparison. To change
/// this setting, construct your handler with `Etag::new().strong()`.
/// See [`etag::EntityTag`](https://docs.rs/etag/3.0.0/etag/struct.EntityTag.html#comparison)
/// for further documentation.
#[derive(Default, Clone, Copy, Debug)]
pub struct Etag {
    strong: bool,
}

impl Etag {
    /// constructs a new Etag handler
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures this handler to use strong content-based etag
    /// comparison only. See
    /// [`etag::EntityTag`](https://docs.rs/etag/3.0.0/etag/struct.EntityTag.html#comparison)
    /// for further documentation on the differences between strong
    /// and weak etag comparison.
    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }
}

impl Handler for Etag {
    async fn run(&self, conn: Conn) -> Conn {
        conn
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        // RFC 9110 §13.1.2: `If-None-Match: *` matches any current representation. When the
        // response carries one (a successful status with a body), the precondition fails and
        // we respond `304 Not Modified`.
        if conn.request_headers().get_str(KnownHeaderName::IfNoneMatch) == Some("*") {
            if conn.response_body().is_some() && conn.status().is_none_or(|s| s.is_success()) {
                return conn.with_status(Status::NotModified);
            }
            return conn;
        }

        let if_none_match = conn.if_none_match();

        let etag = conn.etag().or_else(|| {
            let etag = conn
                .response_body()
                .and_then(|body| body.static_bytes())
                .map(EntityTag::from_data);

            if let Some(ref entity_tag) = etag {
                conn.set_etag(entity_tag);
            }

            etag
        });

        if let (Some(ref etag), Some(ref if_none_match)) = (etag, if_none_match) {
            let eq = if self.strong {
                etag.strong_eq(if_none_match)
            } else {
                etag.weak_eq(if_none_match)
            };

            if eq {
                return conn.with_status(Status::NotModified);
            }
        }

        conn
    }
}
