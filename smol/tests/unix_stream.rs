#![cfg(unix)]
use test_harness::test;
use trillium_client::Client;
use trillium_smol::{SmolTransport, config};
use trillium_testing::harness;

#[test(harness)]
async fn smoke() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("socket");

    let handle = config().with_host(path.to_str().unwrap()).spawn("ok");
    handle.info().await;

    let stream = SmolTransport::connect_unix(path).await.unwrap();
    let mut conn = Client::new(stream).get("http://localhost/").await.unwrap();

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.response_body().read_string().await.unwrap(), "ok");
    handle.shut_down().await;
}
