use trillium_client::Client;
use trillium_smol::ClientConfig;
use trillium_testing::{assert_ok, with_server};

pub fn main() {
    with_server("ok", |url| async move {
        let client = Client::new(ClientConfig {
            nodelay: Some(true),
            ttl: None,
        });

        assert_eq!(
            "ok",
            client
                .get(url.clone())
                .await?
                .success()?
                .response_body()
                .read_string()
                .await?
        );

        let client = Client::new(ClientConfig::new().with_nodelay(true));

        assert_eq!(
            "ok",
            client
                .get(url.clone())
                .await?
                .success()?
                .response_body()
                .read_string()
                .await?
        );

        assert_ok!(client.get(url).await?);

        Ok(())
    })
}
