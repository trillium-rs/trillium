#[cfg(unix)]
pub fn main() {
    use trillium_static::{crate_relative_path, files};
    trillium_smol::run((
        trillium_logger::logger(),
        files(crate_relative_path!("examples/files")).with_index_file("index.html"),
    ))
}

#[cfg(not(unix))]
pub fn main() {}
