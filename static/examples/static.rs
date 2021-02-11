use myco_static::Static;

pub fn main() {
    myco_smol_server::run("localhost:8000", (), Static::new("/", "../docs/book/"))
}
