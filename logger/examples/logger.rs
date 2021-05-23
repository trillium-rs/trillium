pub fn main() {
    env_logger::init();
    trillium_smol_server::run((trillium_logger::DevLogger, "ok"));
}
