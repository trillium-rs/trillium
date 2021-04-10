use trillium_static_compiled::{include_dir, StaticCompiled};
pub fn main() {
    trillium_smol_server::run(StaticCompiled::new(include_dir!("./src")).with_index_file("lib.rs"));
}
