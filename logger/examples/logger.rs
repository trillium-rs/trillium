use trillium_logger::Logger;

pub fn main() {
    env_logger::init();
    trillium_smol::run((Logger::new(), "ok"));
}
