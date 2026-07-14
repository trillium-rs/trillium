use std::time::{Duration, SystemTime};
use trillium::{Conn, KnownHeaderName, Status};
use trillium_caching_headers::{CachingHeaders, CachingHeadersExt, Modified};
use trillium_testing::{TestServer, harness, test};

/// An arbitrary fixed instant, and a timestamp comfortably after it. A client
/// presenting `later` has "seen" a representation at least as new as one stamped
/// `EPOCH_ISH`, so `If-Modified-Since: later` alone means "not modified".
const EPOCH_ISH: SystemTime = SystemTime::UNIX_EPOCH;

fn later() -> String {
    httpdate::fmt_http_date(EPOCH_ISH + Duration::from_secs(60 * 60))
}

/// A handler that stamps a `Last-Modified` which never moves, and a body.
async fn last_modified(conn: Conn) -> Conn {
    conn.with_last_modified(EPOCH_ISH)
        .with_status(Status::Ok)
        .with_body("hello")
}

#[test(harness)]
async fn if_modified_since_alone_still_yields_not_modified() {
    let app = TestServer::new((Modified::new(), last_modified)).await;

    app.get("/")
        .with_request_header(KnownHeaderName::IfModifiedSince, later())
        .await
        .assert_status(304);
}

#[test(harness)]
async fn if_none_match_suppresses_if_modified_since() {
    // RFC 9110 §13.1.3: If-None-Match wholly replaces If-Modified-Since.
    //
    // This is the case that matters. The entity tag does *not* match — the body
    // is genuinely different from what the client holds — but `Last-Modified` is
    // older than `If-Modified-Since`, so a handler evaluating both conditions
    // would answer 304 and the client would keep rendering a stale body.
    let app = TestServer::new((CachingHeaders::new(), last_modified)).await;

    app.get("/")
        .with_request_header(KnownHeaderName::IfNoneMatch, r#""not-the-current-etag""#)
        .with_request_header(KnownHeaderName::IfModifiedSince, later())
        .await
        .assert_status(200);
}

#[test(harness)]
async fn if_none_match_that_matches_still_yields_not_modified() {
    // The other half of the precedence rule: when the tag *does* match, the 304
    // comes from the entity tag rather than from the timestamp.
    let app = TestServer::new((CachingHeaders::new(), last_modified)).await;

    let conn = app.get("/").await;
    conn.assert_status(200);
    let etag = conn.response_headers().get_str("etag").unwrap().to_string();

    app.get("/")
        .with_request_header(KnownHeaderName::IfNoneMatch, etag)
        .with_request_header(KnownHeaderName::IfModifiedSince, later())
        .await
        .assert_status(304);
}

#[test(harness)]
async fn unparseable_if_none_match_still_suppresses_if_modified_since() {
    // The spec conditions on the *presence* of the field, not on our ability to
    // parse it. `*` is a valid If-None-Match that is not a valid entity tag, and
    // it must not fall back to the timestamp comparison.
    //
    // Here the wildcard genuinely does match the current representation, so 304
    // is correct — but it must come from Etag's wildcard rule, not from Modified.
    let app = TestServer::new((Modified::new(), last_modified)).await;

    app.get("/")
        .with_request_header(KnownHeaderName::IfNoneMatch, "*")
        .with_request_header(KnownHeaderName::IfModifiedSince, later())
        .await
        // Modified alone, with no Etag handler in the stack, must not answer the
        // conditional at all.
        .assert_status(200);
}
