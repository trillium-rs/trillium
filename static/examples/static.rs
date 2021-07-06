use trillium_static::{crate_relative_path, StaticFileHandler};

pub fn main() {
    trillium_smol::run((
        trillium_logger::Logger::new().with_target(trillium_logger::Target::Stdout),
        StaticFileHandler::new(crate_relative_path!("examples/files"))
            .with_index_file("index.html"),
    ));
}
