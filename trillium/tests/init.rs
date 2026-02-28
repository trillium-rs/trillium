use std::io;
use test_harness::test;
use trillium::Handler;
use trillium_client::Client;
use trillium_http::ServerConfig;
use trillium_testing::{ServerConnector, TestResult, harness};

async fn test_client(mut handler: impl Handler) -> Client {
    let mut info = ServerConfig::default().into();
    handler.init(&mut info).await;
    let connector = ServerConnector::new(handler).with_server_config(info.into());
    Client::new(connector).with_base("http://test.host")
}

#[test(harness)]
async fn init_doctest() -> TestResult {
    use trillium::{Conn, Init};

    #[derive(Debug, Clone)]
    struct MyDatabaseConnection(&'static str);
    impl MyDatabaseConnection {
        async fn connect(uri: &'static str) -> io::Result<Self> {
            Ok(Self(uri))
        }

        async fn query(&self, query: &str) -> String {
            format!("you queried `{}` against {}", query, &self.0)
        }
    }

    let client = test_client((
        Init::new(|info| async move {
            let db = MyDatabaseConnection::connect("mydatabase://...")
                .await
                .unwrap();
            info.with_state(db)
        }),
        |conn: Conn| async move {
            let Some(db) = conn.shared_state::<MyDatabaseConnection>() else {
                return conn.with_status(500);
            };
            let response = db.query("select * from users limit 1").await;
            conn.ok(response)
        },
    ))
    .await;

    let mut conn = client.get("/").await?;

    assert_eq!(
        conn.response_body().read_string().await?,
        "you queried `select * from users limit 1` against mydatabase://..."
    );

    Ok(())
}
