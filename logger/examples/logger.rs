use myco::sequence;
use myco_logger::DevLogger;

pub fn main() {
    env_logger::init();
    myco_smol_server::run(sequence![DevLogger, "ok"]);
}
