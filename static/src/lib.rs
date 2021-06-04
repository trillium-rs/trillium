#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
Serves static file assets from the file system.

```
use trillium_static::{StaticFileHandler, crate_relative_path};

let handler = StaticFileHandler::new(crate_relative_path!("examples/files"))
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
    "<h1>hello world</h1>",
    "content-type" => "text/html"
);
assert_not_handled!(get("/file_that_does_not_exist.txt").on(&handler));
assert_ok!(get("/index.html").on(&handler));
assert_ok!(get("/subdir/index.html").on(&handler), "subdir index.html");
assert_ok!(get("/subdir").on(&handler), "subdir index.html");
assert_not_handled!(get("/subdir_with_no_index").on(&handler));
assert_ok!(
    get("/subdir_with_no_index/plaintext.txt").on(&handler),
    "plaintext file",
    "content-type" => "text/plain"
);


// with a different index file
let plaintext_index = StaticFileHandler::new(crate_relative_path!("examples/files"))
    .with_index_file("plaintext.txt");

assert_not_handled!(get("/").on(&plaintext_index));
assert_not_handled!(get("/subdir").on(&plaintext_index));
assert_ok!(
    get("/subdir_with_no_index").on(&plaintext_index),
    "plaintext file",
    "content-type" => "text/plain"
);

```

## stability note

Please note that this crate is fairly incomplete, while functional. It
does not include any notion of range requests or cache headers. It
serves all files from disk every time, with no in-memory caching.

It's also worth noting that this currently uses `async_fs`. Adding
support for other async fs libraries (tokio fs, async-std fs) is in
future plans.

*/
use async_fs::File;
use futures_lite::io::BufReader;
pub use relative_path;
use std::path::{Path, PathBuf};
use trillium::{
    async_trait, conn_unwrap,
    http_types::{content::ContentType, Body},
    Conn, Handler,
};

/**
trillium handler to serve static files from the filesystem
*/
#[derive(Debug)]
pub struct StaticFileHandler {
    fs_root: PathBuf,
    index_file: Option<String>,
}

#[derive(Debug)]
enum Record {
    File(PathBuf, File, u64),
    Dir(PathBuf),
}

impl StaticFileHandler {
    async fn resolve_fs_path(&self, url_path: &str) -> Option<PathBuf> {
        let mut file_path = self.fs_root.clone();
        for segment in Path::new(url_path) {
            match segment.to_str() {
                Some("/") => {}
                Some(".") => {}
                Some("..") => {
                    file_path.pop();
                }
                _ => {
                    file_path.push(segment);
                }
            };
        }

        if file_path.starts_with(&self.fs_root) {
            async_fs::canonicalize(file_path).await.ok()
        } else {
            None
        }
    }

    async fn resolve(&self, url_path: &str) -> Option<Record> {
        let fs_path = self.resolve_fs_path(url_path).await?;
        let metadata = async_fs::metadata(&fs_path).await.ok()?;
        if metadata.is_dir() {
            Some(Record::Dir(fs_path))
        } else if metadata.is_file() {
            let len = metadata.len();
            File::open(&fs_path)
                .await
                .ok()
                .map(|file| Record::File(fs_path, file, len))
        } else {
            None
        }
    }

    fn serve_file(mut conn: Conn, path: PathBuf, file: File, len: u64) -> Conn {
        if let Some(mime) = path.to_str().and_then(mime_db::lookup) {
            conn.headers_mut().apply(ContentType::new(mime));
        }

        conn.ok(Body::from_reader(BufReader::new(file), Some(len)))
    }

    /**
    builds a new StaticFileHandler

    ```
    use trillium_static::{StaticFileHandler, crate_relative_path};
    let handler = StaticFileHandler::new(crate_relative_path!("examples/files"));

    use trillium_testing::prelude::*;

    assert_not_handled!(get("/").on(&handler)); // no index file configured

    assert_ok!(
        get("/index.html").on(&handler),
        "<h1>hello world</h1>",
        "content-type" => "text/html"
    );
    ```
    */
    pub fn new(fs_root: impl Into<PathBuf>) -> Self {
        let fs_root = fs_root.into().canonicalize().unwrap();
        log::info!("serving {:?}", &fs_root);
        Self {
            fs_root,
            index_file: None,
        }
    }

    /**
    sets the index file on this StaticFileHandler
    ```
    use trillium_static::{StaticFileHandler, crate_relative_path};

    let handler = StaticFileHandler::new(crate_relative_path!("examples/files"))
        .with_index_file("index.html");

    use trillium_testing::prelude::*;
    assert_ok!(get("/").on(&handler), "<h1>hello world</h1>", "content-type" => "text/html");
    ```
    */
    pub fn with_index_file(mut self, file: &str) -> Self {
        self.index_file = Some(file.to_string());
        self
    }
}

#[async_trait]
impl Handler for StaticFileHandler {
    async fn run(&self, conn: Conn) -> Conn {
        match self.resolve(conn.path()).await {
            Some(Record::File(path, file, len)) => Self::serve_file(conn, path, file, len),

            Some(Record::Dir(path)) => {
                let index = conn_unwrap!(conn, self.index_file.as_ref());
                let path = path.join(index);
                let metadata = conn_unwrap!(conn, async_fs::metadata(&path).await.ok());
                let file = conn_unwrap!(conn, File::open(path.to_str().unwrap()).await.ok());
                Self::serve_file(conn, path, file, metadata.len())
            }

            _ => conn,
        }
    }
}

/// a convenient helper macro to build a str relative to the crate root
#[macro_export]
macro_rules! crate_relative_path {
    ($path:literal) => {
        $crate::relative_path::RelativePath::new($path).to_logical_path(env!("CARGO_MANIFEST_DIR"))
    };
}
