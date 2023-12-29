use test_harness::test;
use trillium_client::{
    Client,
    KnownHeaderName::{Accept, Connection, Host, UserAgent},
};
use trillium_http::Headers;
use trillium_testing::{harness, ServerConnector, TestResult};

#[test(harness = harness)]
async fn default_headers() -> TestResult {
    let client = Client::new(ServerConnector::new("ok"))
        .with_default_header(UserAgent, "overridden")
        .without_default_header(Accept);

    let conn = client.get("http://_").without_header(UserAgent).await?;

    assert_eq!(
        conn.request_headers(),
        &Headers::from_iter([(Host, "_"), (Connection, "close")])
    );

    let conn = client.get("http://_").with_header(Accept, "*/*").await?;
    assert_eq!(
        conn.request_headers(),
        &Headers::from_iter([
            (UserAgent, "overridden"),
            (Host, "_"),
            (Accept, "*/*"),
            (Connection, "close")
        ])
    );

    Ok(())
}
