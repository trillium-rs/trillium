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
Serves static file assets from memory, as included in the binary at
compile time. Because this includes file system content at compile
time, it requires a macro interface, [`static_compiled`](crate::static_compiled).

If the root is a directory, it will recursively serve any files
relative to the path that this handler is mounted at, or an index file
if one is configured with
[`with_index_file`](crate::StaticCompiledHandler::with_index_file).

If the root is a file, it will serve that file at all request paths.

This crate contains code from [`include_dir`][include_dir], but with
several tweaks to make it more suitable for this specific use case.

[include_dir]:https://docs.rs/include_dir/latest/include_dir/

```
# #[cfg(not(unix))] fn main() {}
# #[cfg(unix)] fn main() {
use trillium_static_compiled::static_compiled;

let handler = static_compiled!("./examples/files")
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

assert_ok!(
    get("/").on(&handler),
    "<html>\n  <head>\n    <script src=\"/js.js\"></script>\n  </head>\n  <body>\n    <h1>hello world</h1>\n  </body>\n</html>",
    "content-type" => "text/html"
);
assert_not_handled!(get("/file_that_does_not_exist.txt").on(&handler));
assert_ok!(get("/index.html").on(&handler));
assert_ok!(
    get("/subdir/index.html").on(&handler),
    "subdir index.html 🎈",
    "content-type" => "text/html; charset=utf-8"
);
assert_ok!(get("/subdir").on(&handler), "subdir index.html 🎈");
assert_not_handled!(get("/subdir_with_no_index").on(&handler));
assert_ok!(
    get("/subdir_with_no_index/plaintext.txt").on(&handler),
    "plaintext file",
    "content-type" => "text/plain"
);


// with a different index file
let plaintext_index = static_compiled!("./examples/files")
    .with_index_file("plaintext.txt");

assert_not_handled!(get("/").on(&plaintext_index));
assert_not_handled!(get("/subdir").on(&plaintext_index));
assert_ok!(
    get("/subdir_with_no_index").on(&plaintext_index),
    "plaintext file",
    "content-type" => "text/plain"
);

// with no index file
let no_index = static_compiled!("./examples/files");

assert_not_handled!(get("/").on(&no_index));
assert_not_handled!(get("/subdir").on(&no_index));
assert_not_handled!(get("/subdir_with_no_index").on(&no_index));
# }
```
*/

use trillium::{
    Conn, Handler,
    KnownHeaderName::{ContentType, LastModified},
};

mod dir;
mod dir_entry;
mod file;
mod metadata;

pub(crate) use crate::{dir::Dir, dir_entry::DirEntry, file::File, metadata::Metadata};

#[doc(hidden)]
pub mod __macro_internals {
    pub use crate::{dir::Dir, dir_entry::DirEntry, file::File, metadata::Metadata};
    pub use trillium_static_compiled_macros::{include_dir, include_entry};
}

/**
The static compiled handler which contains the compile-time loaded
assets

*/
#[derive(Debug, Clone, Copy)]
pub struct StaticCompiledHandler {
    root: DirEntry,
    index_file: Option<&'static str>,
}

impl StaticCompiledHandler {
    /// Constructs a new StaticCompiledHandler. This must be used in
    /// conjunction with [`root!`](crate::root). See crate-level docs for
    /// example usage.
    pub fn new(root: DirEntry) -> Self {
        Self {
            root,
            index_file: None,
        }
    }

    /// Configures the optional index file for this
    /// StaticCompiledHandler. See the crate-level docs for example
    /// usage.
    pub fn with_index_file(mut self, file: &'static str) -> Self {
        if let Some(file) = self.root.as_file() {
            panic!(
                "root is a file ({:?}), with_index_file is not meaningful.",
                file.path()
            );
        }
        self.index_file = Some(file);
        self
    }

    fn serve_file(&self, mut conn: Conn, file: File) -> Conn {
        let mime = mime_guess::from_path(file.path()).first_or_text_plain();

        let is_ascii = file.contents().is_ascii();
        let is_text = matches!(
            (mime.type_(), mime.subtype()),
            (mime::APPLICATION, mime::JAVASCRIPT) | (mime::TEXT, _) | (_, mime::HTML)
        );

        conn.response_headers_mut().try_insert(
            ContentType,
            if is_text && !is_ascii {
                format!("{mime}; charset=utf-8")
            } else {
                mime.to_string()
            },
        );

        if let Some(metadata) = file.metadata() {
            conn.response_headers_mut()
                .try_insert(LastModified, httpdate::fmt_http_date(metadata.modified()));
        }

        conn.ok(file.contents())
    }

    fn get_item(&self, path: &str) -> Option<DirEntry> {
        if path.is_empty() || self.root.is_file() {
            Some(self.root)
        } else {
            self.root.as_dir().and_then(|dir| {
                dir.get_dir(path)
                    .copied()
                    .map(DirEntry::Dir)
                    .or_else(|| dir.get_file(path).copied().map(DirEntry::File))
            })
        }
    }
}

impl Handler for StaticCompiledHandler {
    async fn run(&self, conn: Conn) -> Conn {
        match (
            self.get_item(conn.path().trim_start_matches('/')),
            self.index_file,
        ) {
            (None, _) => conn,
            (Some(DirEntry::File(file)), _) => self.serve_file(conn, file),
            (Some(DirEntry::Dir(_)), None) => conn,
            (Some(DirEntry::Dir(dir)), Some(index_file)) => {
                if let Some(file) = dir.get_file(dir.path().join(index_file)) {
                    self.serve_file(conn, *file)
                } else {
                    conn
                }
            }
        }
    }
}

/**
The preferred interface to build a StaticCompiledHandler

Macro interface to build a
[`StaticCompiledHandler`]. `static_compiled!("assets")` is
identical to
`StaticCompiledHandler::new(root!("assets"))`.

This takes one argument, which must be a string literal.

## Relative paths

Relative paths are expanded and canonicalized relative to
`$CARGO_MANIFEST_DIR`, which is usually the directory that contains
your Cargo.toml. If compiled within a workspace, this will be the
subcrate's Cargo.toml.

## Environment variable expansion

If the argument to `static_compiled` contains substrings that are
formatted like an environment variable, beginning with a $, they will
be interpreted in the compile time environment.

For example "$OUT_DIR/some_directory" will expand to the directory
`some_directory` within the env variable `$OUT_DIR` set by cargo. See
[this link][env_vars] for further documentation on the environment
variables set by cargo.

[env_vars]:https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
*/

#[macro_export]
macro_rules! static_compiled {
    ($path:tt) => {
        $crate::StaticCompiledHandler::new($crate::root!($path))
    };
}

/**
Include the path as root. To be passed into [`StaticCompiledHandler::new`].

This takes one argument, which must be a string literal.

## Relative paths

Relative paths are expanded and canonicalized relative to
`$CARGO_MANIFEST_DIR`, which is usually the directory that contains
your Cargo.toml. If compiled within a workspace, this will be the
subcrate's Cargo.toml.

## Environment variable expansion

If the argument to `static_compiled` contains substrings that are
formatted like an environment variable, beginning with a $, they will
be interpreted in the compile time environment.

For example "$OUT_DIR/some_directory" will expand to the directory
`some_directory` within the env variable `$OUT_DIR` set by cargo. See
[this link][env_vars] for further documentation on the environment
variables set by cargo.

[env_vars]:https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates

*/
#[macro_export]
macro_rules! root {
    ($path:tt) => {{
        use $crate::__macro_internals::{include_entry, Dir, DirEntry, File, Metadata};
        const ENTRY: DirEntry = include_entry!($path);
        ENTRY
    }};
}
