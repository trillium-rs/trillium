use trillium::{Conn, Status};
use trillium_caching_headers::{CachingHeadersExt, EntityTag, Etag};
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

#[test(harness)]
async fn error_responses_are_not_rewritten_to_304() {
    // RFC 9110 §13.2.1: preconditions apply only to responses that would otherwise be
    // successful. A 500 carrying an etag that matches If-None-Match must stay a 500.
    async fn error(conn: Conn) -> Conn {
        conn.with_status(Status::InternalServerError)
            .with_body("internal error")
            .with_etag(&EntityTag::from_data(b"internal error"))
    }
    let app = TestServer::new((Etag::new(), error)).await;

    app.get("/")
        .with_request_header(
            "if-none-match",
            EntityTag::from_data(b"internal error").to_string(),
        )
        .await
        .assert_status(500)
        .assert_body("internal error");
}

#[test(harness)]
async fn redirects_keep_their_location() {
    async fn redirect(conn: Conn) -> Conn {
        conn.with_status(Status::MovedPermanently)
            .with_response_header("location", "/elsewhere")
            .with_etag(&EntityTag::from_data(b"moved"))
            .with_body("moved")
    }
    let app = TestServer::new((Etag::new(), redirect)).await;

    let conn = app
        .get("/")
        .with_request_header("if-none-match", EntityTag::from_data(b"moved").to_string())
        .await;
    conn.assert_status(301);
    assert_eq!(
        conn.response_headers().get_str("location"),
        Some("/elsewhere")
    );
}

#[test(harness)]
async fn no_etag_is_generated_for_unsuccessful_responses() {
    async fn error(conn: Conn) -> Conn {
        conn.with_status(Status::InternalServerError)
            .with_body("internal error")
    }
    let app = TestServer::new((Etag::new(), error)).await;

    let conn = app.get("/").await;
    conn.assert_status(500);
    assert!(conn.response_headers().get_str("etag").is_none());
}

#[test(harness)]
async fn matching_if_none_match_on_a_write_method_passes_through() {
    // RFC 9110 §13.1.2: a false If-None-Match yields 304 only for GET and HEAD. By
    // before_send the mutation has already run, so the completed response stands.
    async fn create(conn: Conn) -> Conn {
        conn.with_status(Status::Ok)
            .with_body("resource created")
            .with_etag(&EntityTag::from_data(b"resource created"))
    }
    let app = TestServer::new((Etag::new(), create)).await;

    app.post("/")
        .with_request_header(
            "if-none-match",
            EntityTag::from_data(b"resource created").to_string(),
        )
        .await
        .assert_status(200)
        .assert_body("resource created");
}

#[test(harness)]
async fn if_none_match_wildcard_does_not_apply_to_write_methods() {
    let app = TestServer::new((Etag::new(), "hello")).await;

    app.post("/")
        .with_request_header("if-none-match", "*")
        .await
        .assert_status(200)
        .assert_body("hello");
}
