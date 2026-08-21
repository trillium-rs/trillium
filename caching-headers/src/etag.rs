use crate::CachingHeadersExt;
use etag::EntityTag;
use trillium::{Conn, Handler, KnownHeaderName, Method, Status};

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
/// The conditional comparison applies only to `GET` and `HEAD` requests with successful
/// responses; responses to other methods, error responses, and redirects pass through unchanged,
/// and only successful responses receive a generated etag. Enforcing preconditions on
/// state-changing requests — refusing the write with `412 Precondition Failed` — requires
/// knowing the resource's entity tag before the write runs, so it is up to the application.
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
        // RFC 9110 §13.2.1: preconditions apply only when the response would otherwise be
        // successful, and §13.1.2 answers a matching `If-None-Match` with `304 Not Modified`
        // only for GET and HEAD. By `before_send` any other method has already run, so the
        // only safe treatment of its response is to pass it through untouched.
        let successful = conn.status().is_none_or(|status| status.is_success());
        let preconditions_apply = successful && matches!(conn.method(), Method::Get | Method::Head);

        // `If-None-Match: *` matches any current representation (a body).
        if conn.request_headers().get_str(KnownHeaderName::IfNoneMatch) == Some("*") {
            if preconditions_apply && conn.response_body().is_some() {
                return conn.with_status(Status::NotModified);
            }
            return conn;
        }

        let if_none_match = conn.if_none_match();

        let etag = conn.etag().or_else(|| {
            // a generated entity tag on an error or redirect body would invite caches to
            // revalidate against a representation that isn't the resource
            if !successful {
                return None;
            }

            let etag = conn
                .response_body()
                .and_then(|body| body.static_bytes())
                .map(EntityTag::from_data);

            if let Some(ref entity_tag) = etag {
                conn.set_etag(entity_tag);
            }

            etag
        });

        if !preconditions_apply {
            return conn;
        }

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
