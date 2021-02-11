use myco_static_compiled::{include_dir, IndexBehavior, StaticCompiled};
pub fn main() {
    let handler = StaticCompiled::new(include_dir!("../docs/book"))
        .with_index_behavior(IndexBehavior::File("index.html"));
    myco_smol_server::run("localhost:8000", (), handler);
}
