use myco_static::Static;

pub fn main() {
    myco_smol_server::run(Static::new("../docs/book/"))
}
