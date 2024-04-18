use trillium::Swansong;
use trillium_tokio::config;

#[tokio::test]
async fn spawn_async() {
    config().with_port(0).spawn(()).shut_down().await;
}

#[test]
fn spawn_block() {
    config().with_port(0).spawn(()).shut_down().block();
}

#[test]
fn run() {
    let swansong = Swansong::new();
    swansong.shut_down();
    config().with_port(0).with_swansong(swansong).run(());
}

#[tokio::test]
async fn run_async() {
    let swansong = Swansong::new();
    swansong.shut_down();
    config()
        .with_port(0)
        .with_swansong(swansong)
        .run_async(())
        .await;
}
