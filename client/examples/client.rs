use async_io::Timer;
use async_net::TcpStream;
use myco_client::{Client, Rustls};
use std::time::Duration;

pub fn main() {
    async_global_executor::block_on(async {
        env_logger::init();

        let client = Client::<Rustls<TcpStream>>::new().with_default_pool();

        for _ in 0..5 {
            let client = client.clone();
            async_global_executor::spawn(async move {
                loop {
                    let mut conn = client.get("http://localhost:8011");
                    conn.send().await.unwrap();
                    println!("{:#?}", conn);
                    conn.recycle().await;
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
