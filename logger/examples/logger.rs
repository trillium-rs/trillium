pub fn main() {
    env_logger::init();
    trillium_smol::run((trillium_logger::DevLogger, "ok"));
}
