use trillium_static_compiled::{include_dir, StaticCompiledHandler};
pub fn main() {
    #[cfg(unix)]
    trillium_smol::run((
        trillium_logger::Logger::new(),
        StaticCompiledHandler::new(include_dir!("./examples/files")).with_index_file("index.html"),
    ));
}
