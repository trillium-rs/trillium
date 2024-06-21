use std::time::Duration;
use trillium_client::Client;
use trillium_testing::{client_config, runtime, RuntimeTrait};

async fn handler(conn: trillium::Conn) -> trillium::Conn {
    if conn.path() == "/slow" {
        runtime().delay(Duration::from_secs(5)).await;
    }
    conn.ok("ok")
}

#[test]
fn timeout_on_conn() {
    let _ = env_logger::builder().is_test(true).try_init();
    trillium_testing::with_server(handler, move |url| async move {
        let client = Client::new(client_config()).with_base(url);
        let err = client
            .get("/slow")
            .with_timeout(Duration::from_millis(100))
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "Conn took longer than 100ms");

        assert!(
            client
                .get("/")
                .with_timeout(Duration::from_millis(100))
                .await
                .is_ok()
        );

        Ok(())
    })
}

#[test]
fn timeout_on_client() {
    let _ = env_logger::builder().is_test(true).try_init();

    trillium_testing::with_server(handler, move |url| async move {
        let client = Client::new(client_config())
            .with_base(url)
            .with_timeout(Duration::from_millis(100));
        let err = client.get("/slow").await.unwrap_err();
        assert_eq!(err.to_string(), "Conn took longer than 100ms");

        assert!(client.get("/").await.is_ok());
        Ok(())
    })
}
