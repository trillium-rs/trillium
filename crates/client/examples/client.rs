use trillium_client::Client;
use trillium_smol::ClientConfig;

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    async_global_executor::block_on(async {
        env_logger::init();
        let client = Client::new(ClientConfig::default()).with_default_pool();
        let response_body = client
            .get("http://neverssl.com/")
            .await?
            .success()
            .map_err(|e| e.to_string())?
            .response_body()
            .await?;

        println!("{response_body}");
        Ok(())
    })
}
