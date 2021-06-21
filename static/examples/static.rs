use trillium_static::{crate_relative_path, StaticFileHandler};

pub fn main() {
    env_logger::init();
    trillium_smol::run(
        StaticFileHandler::new(crate_relative_path!("examples/files"))
            .with_index_file("index.html"),
    )
}
