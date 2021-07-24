use trillium_static_compiled::{include_dir, StaticCompiledHandler};
pub fn main() {
    #[cfg(unix)]
    trillium_smol::run(
        StaticCompiledHandler::new(include_dir!("./examples/files")).with_index_file("lib.rs"),
    );
}
