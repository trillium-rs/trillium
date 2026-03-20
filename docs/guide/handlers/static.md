## Static file serving

Trillium offers two rudimentary approaches to static file serving for now. Neither of these approaches perform any cache-related header checking yet.

### From disk
This handler loads content from disk at request, and does not yet do any in-memory caching.

[rustdocs (main)](https://docs.trillium.rs/trillium_static/index.html)


```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-static = { path = "../static" }
# trillium-logger = { path = "../logger" }
# file: static/examples/files/
#
pub fn main() {
    use trillium_static::{crate_relative_path, files};
    trillium_smol::run((
        trillium_logger::logger(),
        files(crate_relative_path!("files")).with_index_file("index.html"),
    ))
}
```

### From memory, at compile time
This handler includes all of the static content in the compiled binary, allowing it to be shipped independently from the assets.

[rustdocs (main)](https://docs.trillium.rs/trillium_static_compiled/index.html)

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-static-compiled = { path = "../static-compiled" }
# trillium-logger = { path = "../logger" }
# trillium-caching-headers = { path = "../caching-headers" }
# file: static-compiled/examples/files
#
pub fn main() {
    use trillium_static_compiled::static_compiled;

    trillium_smol::run((
        trillium_logger::Logger::new(),
        trillium_caching_headers::CachingHeaders::new(),
        static_compiled!("./files").with_index_file("index.html"),
    ));
}
```
