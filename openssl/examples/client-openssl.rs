use trillium_client::Client;
use trillium_openssl::OpenSslConfig;
use trillium_smol::{ClientConfig, async_global_executor::block_on};

pub fn main() {
    block_on(async {
        let client = Client::new(OpenSslConfig::<ClientConfig>::default());

        let conn = client.get("https://localhost:8080").await.unwrap();
        println!("{conn:?}");
    });
}
