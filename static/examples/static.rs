use trillium_static::{crate_relative_path, StaticFileHandler};

pub fn main() {
    trillium_smol::run(
        StaticFileHandler::new(crate_relative_path!("examples/files"))
            .with_index_file("index.html"),
    )
}
