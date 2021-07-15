use trillium_forwarding::Forwarding;
use trillium_logger::{apache_common, formatters::secure, Logger, Target};

pub fn main() {
    trillium_smol::run((
        Forwarding::trust_always(),
        Logger::new()
            .with_formatter((secure, " ", apache_common("-", "-")))
            .with_target(Target::Stdout),
        "ok",
    ));
}
