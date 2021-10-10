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

## stability note

Please note that this crate is fairly incomplete, while functional. It
does not include any notion of range requests or cache headers. It
serves all files from disk every time, with no in-memory caching.

This may also merge with the [static file handler](https://docs.trillium.rs/trillium_static/)

```
# #[cfg(not(unix))] fn main() {}
# #[cfg(unix)] fn main() {
use trillium_static_compiled::{include_dir, StaticCompiledHandler};

let handler = StaticCompiledHandler::new(include_dir!("examples/files"))
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
let plaintext_index = StaticCompiledHandler::new(include_dir!("examples/files"))
    .with_index_file("plaintext.txt");

assert_not_handled!(get("/").on(&plaintext_index));
assert_not_handled!(get("/subdir").on(&plaintext_index));
assert_ok!(
    get("/subdir_with_no_index").on(&plaintext_index),
    "plaintext file",
    "content-type" => "text/plain"
);

// with no index file
let no_index = StaticCompiledHandler::new(include_dir!("examples/files"));

assert_not_handled!(get("/").on(&no_index));
assert_not_handled!(get("/subdir").on(&no_index));
assert_not_handled!(get("/subdir_with_no_index").on(&no_index));
# }
```
*/
pub use include_dir::include_dir;
use include_dir::{Dir, DirEntry, File};
use trillium::{async_trait, Conn, Handler, KnownHeaderName::ContentType};
/**
The static compiled handler which contains the compile-time loaded
assets

*/
#[derive(Debug, Clone, Copy)]
pub struct StaticCompiledHandler {
    dir: Dir<'static>,
    index_file: Option<&'static str>,
}

impl StaticCompiledHandler {
    /// Constructs a new StaticCompiledHandler. This must be used in
    /// conjunction with [`include_dir!`]. See crate-level docs for
    /// example usage.
    pub fn new(dir: Dir<'static>) -> Self {
        Self {
            dir,
            index_file: None,
        }
    }
    /// Configures the optional index file for this
    /// StaticCompiledHandler. See the crate-level docs for example
    /// usage.
    pub fn with_index_file(mut self, file: &'static str) -> Self {
        self.index_file = Some(file);
        self
    }

    fn serve_file(&self, mut conn: Conn, file: File<'static>) -> Conn {
        let mime = mime_guess::from_path(file.path()).first_or_text_plain();

        let is_ascii = file.contents().is_ascii();
        let is_text = matches!(
            (mime.type_(), mime.subtype()),
            (mime::APPLICATION, mime::JAVASCRIPT) | (mime::TEXT, _) | (_, mime::HTML)
        );

        conn.headers_mut().try_insert(
            ContentType,
            if is_text && !is_ascii {
                format!("{}; charset=utf-8", mime)
            } else {
                mime.to_string()
            },
        );

        conn.ok(file.contents())
    }

    fn get_item(&self, path: &str) -> Option<DirEntry<'static>> {
        if path.is_empty() {
            Some(DirEntry::Dir(self.dir))
        } else {
            self.dir
                .get_dir(path)
                .map(DirEntry::Dir)
                .or_else(|| self.dir.get_file(path).map(DirEntry::File))
        }
    }
}

#[async_trait]
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
                    self.serve_file(conn, file)
                } else {
                    conn
                }
            }
        }
    }
}

/// Alias for [`StaticCompiledHandler::new`]
pub fn files(dir: Dir<'static>) -> StaticCompiledHandler {
    StaticCompiledHandler::new(dir)
}
