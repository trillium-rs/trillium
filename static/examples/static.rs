use trillium_static::StaticFileHandler;

pub fn main() {
    trillium_smol_server::run(StaticFileHandler::new("examples").with_index_file("index.html"))
}
