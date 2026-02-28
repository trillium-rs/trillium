use test_harness::test;
use trillium_client::Client;
use trillium_testing::{ServerConnector, harness};

#[test(harness)]
async fn test() {
    let client = Client::new(ServerConnector::new("test"));
    let mut conn = client.get("https://example.com/test").await.unwrap();
    assert_eq!(conn.response_body().read_string().await.unwrap(), "test");
}

#[test(harness)]
async fn test_no_dns() {
    let client = Client::new(ServerConnector::new("test"));
    let mut conn = client
        .get("https://not.a.real.tld.example/test")
        .await
        .unwrap();
    assert_eq!(conn.response_body().read_string().await.unwrap(), "test");
}

#[test(harness)]
async fn test_post() {
    let client = Client::new(ServerConnector::new(
        |mut conn: trillium::Conn| async move {
            let body = conn.request_body_string().await.unwrap();
            let response = format!(
                "{} {}://{}{} with body \"{}\"",
                conn.method(),
                if conn.is_secure() { "https" } else { "http" },
                conn.inner().host().unwrap_or_default(),
                conn.path(),
                body
            );

            conn.ok(response)
        },
    ));

    let body = client
        .post("https://example.com/test")
        .with_body("some body")
        .await
        .unwrap()
        .response_body()
        .read_string()
        .await
        .unwrap();

    assert_eq!(
        body,
        "POST https://example.com/test with body \"some body\""
    );
}
