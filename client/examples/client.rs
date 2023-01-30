use async_io::Timer;
use std::time::Duration;
use trillium_client::Client;
use trillium_rustls::RustlsConnector;
use trillium_smol::TcpConnector;

type HttpClient = Client<RustlsConnector<TcpConnector>>;

pub fn main() {
    async_global_executor::block_on(async {
        env_logger::init();

        let client = HttpClient::new().with_default_pool();

        for _ in 0..5 {
            let client = client.clone();
            async_global_executor::spawn(async move {
                loop {
                    let mut conn = client.post("http://localhost:8011/").with_body("body");

                    conn.send().await.unwrap();
                    println!("{conn:#?}");
                    Timer::after(Duration::from_millis(fastrand::u64(0..1000))).await;
                }
            })
            .detach();
        }

        loop {
            Timer::after(Duration::from_secs(10)).await;
            dbg!(&client);
        }
    });
}
