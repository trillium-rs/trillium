use std::time::Duration;
use trillium_client::Client;
use trillium_testing::ClientConfig;

async fn handler(conn: trillium::Conn) -> trillium::Conn {
    if conn.path() == "/slow" {
        trillium_testing::delay(Duration::from_secs(5)).await;
    }
    conn.ok("ok")
}

#[test]
fn timeout_on_conn() {
    trillium_testing::with_server(handler, move |url| async move {
        let client = Client::new(ClientConfig::new()).with_base(url);
        let err = client
            .get("/slow")
            .with_timeout(Duration::from_millis(100))
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "Conn took longer than 100ms");

        assert!(client
            .get("/")
            .with_timeout(Duration::from_millis(100))
            .await
            .is_ok());

        Ok(())
    })
}

#[test]
fn timeout_on_client() {
    trillium_testing::with_server(handler, move |url| async move {
        let client = Client::new(ClientConfig::new())
            .with_base(url)
            .with_timeout(Duration::from_millis(100));
        let err = client.get("/slow").await.unwrap_err();
        assert_eq!(err.to_string(), "Conn took longer than 100ms");

        assert!(client.get("/").await.is_ok());
        Ok(())
    })
}
