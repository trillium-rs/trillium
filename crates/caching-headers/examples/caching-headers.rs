use trillium_caching_headers::CachingHeaders;
use trillium_static::{crate_relative_path, files};

fn main() {
    trillium_smol::run((
        CachingHeaders::new(),
        files(crate_relative_path!("examples/file")),
    ))
}
