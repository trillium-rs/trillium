use trillium_client::Client;
use trillium_rustls::RustlsConfig;
use trillium_smol::{ClientConfig, async_global_executor::block_on};

pub fn main() {
    block_on(async {
        let client = Client::new(RustlsConfig::<ClientConfig>::default());

        let _ = dbg!(client.get("https://localhost:8080").await.unwrap());
    });
}
