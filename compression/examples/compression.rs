use trillium_compression::compression;
use trillium_logger::logger;
use trillium_router::router;
use trillium_static::{crate_relative_path, files};
use trillium_static_compiled::static_compiled;

fn main() {
    env_logger::init();
    trillium_smol::run((
        logger(),
        compression(),
        router()
            .get("static/*", static_compiled!("."))
            .get("streaming/*", files(crate_relative_path!("."))),
    ))
}
