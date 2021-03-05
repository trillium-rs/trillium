#[async_std::main]
pub async fn main() {
    myco_async_std_server::run_async("hello world").await;
}

pub fn or_if_main_is_not_async() {
    myco_async_std_server::run("hello world");
}
