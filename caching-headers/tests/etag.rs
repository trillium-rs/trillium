use trillium::{Conn, Status};
use trillium_caching_headers::Etag;
use trillium_testing::{TestServer, harness, test};

#[test(harness)]
async fn if_none_match_wildcard_matches_existing_representation() {
    // `If-None-Match: *` matches any current representation, so a successful response with a body
    // becomes `304 Not Modified`.
    let app = TestServer::new((Etag::new(), "hello")).await;

    app.get("/")
        .with_request_header("if-none-match", "*")
        .await
        .assert_status(304);
}

#[test(harness)]
async fn if_none_match_wildcard_passes_through_when_no_representation() {
    // No matching route → no body → the wildcard precondition does not apply.
    async fn not_found(conn: Conn) -> Conn {
        conn.with_status(Status::NotFound)
    }
    let app = TestServer::new((Etag::new(), not_found)).await;

    app.get("/")
        .with_request_header("if-none-match", "*")
        .await
        .assert_status(404);
}

#[test(harness)]
async fn etag_round_trip_still_works() {
    let app = TestServer::new((Etag::new(), "hello")).await;

    // first request: learn the etag
    let conn = app.get("/").await;
    conn.assert_status(200);
    let etag = conn.response_headers().get_str("etag").unwrap().to_string();

    // conditional request with that etag → 304
    app.get("/")
        .with_request_header("if-none-match", etag)
        .await
        .assert_status(304);
}
