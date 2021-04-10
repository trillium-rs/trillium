use trillium::sequence;
use trillium_logger::DevLogger;

pub fn main() {
    env_logger::init();
    trillium_smol_server::run(sequence![DevLogger, "ok"]);
}
