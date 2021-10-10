#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
Serves static file assets from the file system.

```
# #[cfg(not(unix))] fn main() {}
# #[cfg(unix)] fn main() {
use trillium_static::{StaticFileHandler, crate_relative_path};

let mut handler = StaticFileHandler::new(crate_relative_path!("examples/files"))
    .with_index_file("index.html");


// given the following directory layout
//
// examples/files
// ├── index.html
// ├── subdir
// │  └── index.html
// └── subdir_with_no_index
//    └── plaintext.txt
//


use trillium_testing::prelude::*;

init(&mut handler);

assert_ok!(
    get("/").on(&handler),
    "<h1>hello world</h1>",
    "content-type" => "text/html; charset=utf-8"
);
assert_not_handled!(get("/file_that_does_not_exist.txt").on(&handler));
assert_ok!(get("/index.html").on(&handler));
assert_ok!(get("/subdir/index.html").on(&handler), "subdir index.html");
assert_ok!(get("/subdir").on(&handler), "subdir index.html");
assert_not_handled!(get("/subdir_with_no_index").on(&handler));
assert_ok!(
    get("/subdir_with_no_index/plaintext.txt").on(&handler),
    "plaintext file",
    "content-type" => "text/plain; charset=utf-8"
);


// with a different index file
let plaintext_index = StaticFileHandler::new(crate_relative_path!("examples/files"))
    .with_index_file("plaintext.txt");

assert_not_handled!(get("/").on(&plaintext_index));
assert_not_handled!(get("/subdir").on(&plaintext_index));
assert_ok!(
    get("/subdir_with_no_index").on(&plaintext_index),
    "plaintext file",
    "content-type" => "text/plain; charset=utf-8"
);
# }
```


## ❗IMPORTANT❗

this crate has three features currently: `smol`, `async-std`, and
`tokio`.

You **must** enable one of these in order to use the crate. This
is intended to be a temporary situation, and eventually you will not
need to specify the runtime through feature flags.

## stability note

Please note that this crate is fairly incomplete, while functional. It
does not include any notion of range requests or cache headers. It
serves all files from disk every time, with no in-memory caching.
*/

mod fs_shims;
mod handler;
mod options;
mod static_conn_ext;

pub use handler::StaticFileHandler;
pub use relative_path;
pub use static_conn_ext::StaticConnExt;

/// a convenient helper macro to build a str relative to the crate root
#[macro_export]
macro_rules! crate_relative_path {
    ($path:literal) => {
        $crate::relative_path::RelativePath::new($path).to_logical_path(env!("CARGO_MANIFEST_DIR"))
    };
}

/// convenience alias for [`StaticFileHandler::new`]
pub fn files(fs_root: impl AsRef<std::path::Path>) -> StaticFileHandler {
    StaticFileHandler::new(fs_root)
}
