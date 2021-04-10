use trillium_static::Static;

pub fn main() {
    trillium_smol_server::run(Static::new("../docs/book/").with_index_file("index.html"))
}
