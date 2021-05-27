use trillium_static_compiled::{include_dir, StaticCompiledHandler};
pub fn main() {
    trillium_smol_server::run(
        StaticCompiledHandler::new(include_dir!("./examples/files")).with_index_file("lib.rs"),
    );
}
