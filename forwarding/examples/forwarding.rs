use trillium_forwarding::Forwarding;
use trillium_logger::{Logger, apache_common, formatters::secure};

pub fn main() {
    trillium_smol::run((
        Forwarding::trust_always(),
        Logger::new().with_formatter((secure, " ", apache_common("-", "-"))),
        "ok",
    ));
}
