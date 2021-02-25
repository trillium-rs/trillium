use myco_static_compiled::{include_dir, StaticCompiled};
pub fn main() {
    myco_smol_server::run(
        StaticCompiled::new(include_dir!("../docs/book")).with_index_file("index.html"),
    );
}
