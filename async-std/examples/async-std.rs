#[async_std::main]
pub async fn main() {
    env_logger::init();
    trillium_async_std::run_async("hello world").await;
}

pub fn or_if_main_is_not_async() {
    trillium_async_std::run("hello world");
}
