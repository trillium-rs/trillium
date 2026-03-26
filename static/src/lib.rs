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

//! Serves static file assets from the file system.
//!
//! ```
//! # #[cfg(not(unix))] fn main() {}
//! # #[cfg(unix)] fn main() {
//! use trillium_static::{StaticFileHandler, crate_relative_path};
//! use trillium_testing::TestServer;
//!
//! # trillium_testing::block_on(async {
//! let handler = StaticFileHandler::new(crate_relative_path!("examples/files"))
//!     .with_index_file("index.html");
//!
//! // given the following directory layout
//! //
//! // examples/files
//! // ├── index.html
//! // ├── subdir
//! // │  └── index.html
//! // └── subdir_with_no_index
//! //    └── plaintext.txt
//!
//! let app = TestServer::new(handler).await;
//!
//! app.get("/")
//!     .await
//!     .assert_ok()
//!     .assert_body("<h1>hello world</h1>\n")
//!     .assert_header("content-type", "text/html; charset=utf-8");
//!
//! app.get("/file_that_does_not_exist.txt")
//!     .await
//!     .assert_status(404);
//!
//! app.get("/index.html").await.assert_ok();
//!
//! app.get("/subdir/index.html")
//!     .await
//!     .assert_ok()
//!     .assert_body("subdir index.html\n");
//!
//! app.get("/subdir")
//!     .await
//!     .assert_ok()
//!     .assert_body("subdir index.html\n");
//!
//! app.get("/subdir_with_no_index").await.assert_status(404);
//!
//! app.get("/subdir_with_no_index/plaintext.txt")
//!     .await
//!     .assert_ok()
//!     .assert_body("plaintext file\n")
//!     .assert_header("content-type", "text/plain; charset=utf-8");
//!
//! // with a different index file
//! let plaintext_index = StaticFileHandler::new(crate_relative_path!("examples/files"))
//!     .with_index_file("plaintext.txt");
//!
//! let app2 = TestServer::new(plaintext_index).await;
//!
//! app2.get("/").await.assert_status(404);
//! app2.get("/subdir").await.assert_status(404);
//!
//! app2.get("/subdir_with_no_index")
//!     .await
//!     .assert_ok()
//!     .assert_body("plaintext file\n")
//!     .assert_header("content-type", "text/plain; charset=utf-8");
//! # });
//! # }
//! ```
//!
//!
//! ## ❗IMPORTANT❗
//!
//! this crate has three features currently: `smol`, `async-std`, and
//! `tokio`.
//!
//! You **must** enable one of these in order to use the crate. This
//! is intended to be a temporary situation, and eventually you will not
//! need to specify the runtime through feature flags.
//!
//! ## stability note
//!
//! Please note that this crate is fairly incomplete, while functional. It
//! does not include any notion of range requests or cache headers. It
//! serves all files from disk every time, with no in-memory caching.

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

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
