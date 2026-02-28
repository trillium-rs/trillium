use test_harness::test;
use trillium_client::Client;
use trillium_server_common::Config;
use trillium_testing::{RuntimelessClientConfig, RuntimelessServer, TestResult, harness};
#[test(harness)]
async fn round_trip() -> TestResult {
    let handle1 = Config::<RuntimelessServer, ()>::new()
        .with_host("host.com")
        .with_port(80)
        .spawn("server 1");
    handle1.info().await;

    let handle2 = Config::<RuntimelessServer, ()>::new()
        .with_host("other_host.com")
        .with_port(80)
        .spawn("server 2");
    handle2.info().await;

    let client = Client::new(RuntimelessClientConfig::default());
    let mut conn = client.get("http://host.com").await?;
    assert_eq!(conn.response_body().await?, "server 1");

    let mut conn = client.get("http://other_host.com").await?;
    assert_eq!(conn.response_body().await?, "server 2");

    handle1.shut_down().await;
    assert!(client.get("http://host.com").await.is_err());
    assert!(client.get("http://other_host.com").await.is_ok());

    handle2.shut_down().await;
    assert!(client.get("http://other_host.com").await.is_err());

    assert!(RuntimelessServer::is_empty());

    Ok(())
}
