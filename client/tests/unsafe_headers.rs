use test_harness::test;
use trillium_client::{Client, KnownHeaderName};
use trillium_testing::{connector, harness};

#[test(harness)]
async fn bad_characters_in_header_value() {
    assert!(
        Client::new(connector(()))
            .get("http://example.com")
            .with_request_header(
                KnownHeaderName::Referer,
                "x\r\nConnection: keep-alive\r\n\r\nGET / HTTP/1.1\r\nHost: example.com\r\n\r\n"
            )
            .await
            .is_err()
    );
}

#[test(harness)]
async fn bad_characters_in_header_name() {
    assert!(
        Client::new(connector(()))
            .get("http://example.com")
            .with_request_header("dnt: 1\r\nConnection", "keep-alive")
            .await
            .is_err()
    );
}
