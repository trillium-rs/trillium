#[cfg(unix)]
pub fn main() {
    use trillium_static_compiled::static_compiled;

    trillium_smol::run((
        trillium_logger::Logger::new(),
        trillium_caching_headers::CachingHeaders::new(),
        static_compiled!("examples/files").with_index_file("index.html"),
    ));
}

#[cfg(not(unix))]
pub fn main() {}
