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

//! Serves static file assets from memory, as included in the binary at
//! compile time. Because this includes file system content at compile
//! time, it requires a macro interface, [`static_compiled`].
//!
//! If the root is a directory, it will recursively serve any files
//! relative to the path that this handler is mounted at, or an index file
//! if one is configured with
//! [`with_index_file`](crate::StaticCompiledHandler::with_index_file).
//!
//! If the root is a file, it will serve that file at all request paths.
//!
//! ## ETag headers
//!
//! On by default. Each file's source bytes are hashed at compile time via
//! [`etag::EntityTag::from_data`][et] and the resulting tag is emitted as
//! the `ETag` response header on every response. The baked tag is
//! byte-identical to what [`trillium_caching_headers::Etag`][cce] would
//! compute at runtime, so chaining `Etag::new()` after this handler
//! observes the precomputed tag, skips rehashing the body, and handles
//! `If-None-Match` / `304 Not Modified` for free. Opt out per invocation
//! with `static_compiled!("./public", etag = false)`.
//!
//! [et]: https://docs.rs/etag/latest/etag/struct.EntityTag.html#method.from_data
//! [cce]: https://docs.rs/trillium-caching-headers/latest/trillium_caching_headers/struct.Etag.html
//!
//! ## Precompression
//!
//! Optionally pre-compress bundle contents into Brotli, Zstd, and Gzip
//! variants at build time, behind cargo features (`brotli`, `zstd`,
//! `gzip`, or the `compression` meta-feature) and an opt-in macro
//! argument:
//!
//! ```ignore
//! // requires e.g. trillium-static-compiled = { features = ["compression"] }
//! static_compiled!("./public", compress);                  // all enabled encoders
//! static_compiled!("./public", compress = [Brotli, Gzip]); // explicit subset
//! ```
//!
//! Encoders run at maximum quality in parallel via rayon at macro
//! expansion time. Per-file variants are sorted smallest-first and only
//! baked when they save at least 5%; files under 256 bytes are skipped
//! entirely. The handler picks the smallest variant the client's
//! `Accept-Encoding` allows, sets `Content-Encoding`, and emits
//! `Vary: Accept-Encoding` (per-file, only when variants are baked).
//! Composes with [`trillium-compression`][tc], which passes through any
//! response that already has `Content-Encoding` set.
//!
//! [tc]: https://docs.rs/trillium-compression/
//!
//! ## Origin
//!
//! This crate contains code from [`include_dir`][include_dir], but with
//! several tweaks to make it more suitable for this specific use case.
//!
//! [include_dir]:https://docs.rs/include_dir/latest/include_dir/
//!
//! ```
//! use trillium_static_compiled::static_compiled;
//! use trillium_testing::TestServer;
//!
//! # trillium_testing::block_on(async {
//! let handler = static_compiled!("./examples/files").with_index_file("index.html");
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
//! let index = include_str!("../examples/files/index.html");
//! app.get("/")
//!     .await
//!     .assert_ok()
//!     .assert_body(index)
//!     .assert_header("content-type", "text/html");
//!
//! app.get("/file_that_does_not_exist.txt")
//!     .await
//!     .assert_status(404);
//! app.get("/index.html").await.assert_ok();
//!
//! app.get("/subdir/index.html")
//!     .await
//!     .assert_ok()
//!     .assert_body("subdir index.html 🎈\n")
//!     .assert_header("content-type", "text/html; charset=utf-8");
//!
//! app.get("/subdir")
//!     .await
//!     .assert_ok()
//!     .assert_body("subdir index.html 🎈\n");
//!
//! app.get("/subdir_with_no_index").await.assert_status(404);
//!
//! app.get("/subdir_with_no_index/plaintext.txt")
//!     .await
//!     .assert_ok()
//!     .assert_body("plaintext file\n")
//!     .assert_header("content-type", "text/plain");
//!
//! // with a different index file
//! let plaintext_index = static_compiled!("./examples/files").with_index_file("plaintext.txt");
//! let app2 = TestServer::new(plaintext_index).await;
//!
//! app2.get("/").await.assert_status(404);
//! app2.get("/subdir").await.assert_status(404);
//!
//! app2.get("/subdir_with_no_index")
//!     .await
//!     .assert_ok()
//!     .assert_body("plaintext file\n")
//!     .assert_header("content-type", "text/plain");
//!
//! // with no index file
//! let no_index = static_compiled!("./examples/files");
//! let app3 = TestServer::new(no_index).await;
//!
//! app3.get("/").await.assert_status(404);
//! app3.get("/subdir").await.assert_status(404);
//! app3.get("/subdir_with_no_index").await.assert_status(404);
//! # });
//! ```

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use trillium::{
    Conn, Handler, HeaderValues,
    KnownHeaderName::{
        AcceptEncoding, AcceptRanges, ContentEncoding, ContentRange, ContentType, Etag, IfRange,
        LastModified, Range, Vary,
    },
    Status,
};

mod dir;
mod dir_entry;
mod encoding;
mod file;
mod metadata;
mod range;

pub use crate::encoding::Encoding;
pub(crate) use crate::{dir::Dir, dir_entry::DirEntry, file::File, metadata::Metadata};

#[doc(hidden)]
pub mod __macro_internals {
    pub use crate::{
        dir::Dir, dir_entry::DirEntry, encoding::Encoding, file::File, metadata::Metadata,
    };
    pub use trillium_static_compiled_macros::{include_dir, include_entry};
}

fn append_vary_accept_encoding(conn: &mut Conn) {
    let vary = conn
        .response_headers()
        .get_str(Vary)
        .map(|existing| HeaderValues::from(format!("{existing}, Accept-Encoding")))
        .unwrap_or_else(|| HeaderValues::from("Accept-Encoding"));
    conn.response_headers_mut().insert(Vary, vary);
}

/// The static compiled handler which contains the compile-time loaded
/// assets
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

        if let Some(etag) = file.etag() {
            conn.response_headers_mut().try_insert(Etag, etag);
        }

        // Always advertise range support for static content.
        conn.response_headers_mut()
            .try_insert(AcceptRanges, "bytes");

        let total = file.contents().len() as u64;
        let range_spec = conn
            .request_headers()
            .get_str(Range)
            .and_then(range::parse)
            .filter(|_| {
                // If-Range gate: present and matching → honor; present and
                // non-matching → ignore Range and serve full body.
                conn.request_headers()
                    .get_str(IfRange)
                    .is_none_or(|if_range| {
                        range::if_range_matches(
                            if_range,
                            file.etag(),
                            file.metadata().map(Metadata::modified),
                        )
                    })
            });

        if let Some(spec) = range_spec {
            return match range::resolve(spec, total) {
                Some((start, end)) => {
                    let slice = &file.contents()[start as usize..=end as usize];
                    conn.response_headers_mut()
                        .insert(ContentRange, format!("bytes {start}-{end}/{total}"));
                    if !file.encodings().is_empty() {
                        append_vary_accept_encoding(&mut conn);
                    }
                    conn.with_status(Status::PartialContent).with_body(slice)
                }
                None => {
                    conn.response_headers_mut()
                        .insert(ContentRange, format!("bytes */{total}"));
                    conn.with_status(Status::RequestedRangeNotSatisfiable)
                        .with_body("")
                }
            };
        }

        let accept = conn.request_headers().get_str(AcceptEncoding);
        let body = match file.pick_encoding(accept) {
            Some((encoding, bytes)) => {
                conn.response_headers_mut()
                    .insert(ContentEncoding, encoding.token());
                bytes
            }
            None => file.contents(),
        };

        if !file.encodings().is_empty() {
            append_vary_accept_encoding(&mut conn);
        }

        conn.ok(body)
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

/// The preferred interface to build a [`StaticCompiledHandler`].
///
/// `static_compiled!("assets")` is identical to
/// `StaticCompiledHandler::new(root!("assets"))`.
///
/// ## Arguments
///
/// The first argument is a string literal naming a path on disk. After
/// the path, any number of optional arguments may follow in any order,
/// separated by commas:
///
/// ```ignore
/// static_compiled!("./public");                                // defaults
/// static_compiled!("./public", etag = false);                  // skip etag
/// static_compiled!("./public", compress);                      // all enabled encoders
/// static_compiled!("./public", compress = [Brotli, Gzip]);     // specific encoders
/// static_compiled!("./public", compress, etag = false);        // both
/// ```
///
/// ### `etag = bool`
///
/// On by default. When true, an entity tag is computed at compile time
/// for each file's source bytes and emitted as the `ETag` response
/// header. See the [crate-level docs][crate#etag-headers] for details.
///
/// ### `compress` and `compress = [Brotli, Zstd, Gzip]`
///
/// Off by default and gated behind the `brotli` / `zstd` / `gzip` /
/// `compression` cargo features. The bare form `compress` bakes every
/// encoding whose feature is enabled; the listed form bakes a specified
/// subset and is a compile error if a listed encoding's feature is not
/// enabled. See the [crate-level docs][crate#precompression].
///
/// ## Relative paths
///
/// Relative paths are expanded and canonicalized relative to
/// `$CARGO_MANIFEST_DIR`, which is usually the directory that contains
/// your Cargo.toml. If compiled within a workspace, this will be the
/// subcrate's Cargo.toml.
///
/// ## Environment variable expansion
///
/// If the path argument to `static_compiled!` contains substrings that
/// are formatted like an environment variable, beginning with a `$`,
/// they will be interpreted in the compile time environment.
///
/// For example `"$OUT_DIR/some_directory"` will expand to the directory
/// `some_directory` within the env variable `$OUT_DIR` set by cargo. See
/// [this link][env_vars] for further documentation on the environment
/// variables set by cargo.
///
/// [env_vars]:https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
#[macro_export]
macro_rules! static_compiled {
    ($($args:tt)*) => {
        $crate::StaticCompiledHandler::new($crate::root!($($args)*))
    };
}

/// Include the path as root. To be passed into
/// [`StaticCompiledHandler::new`].
///
/// Most callers want [`static_compiled!`] instead, which wraps this in
/// the handler constructor.
///
/// Accepts the same arguments as [`static_compiled!`] — see its
/// documentation for the path-argument grammar, optional `etag` and
/// `compress` arguments, relative-path resolution, and environment
/// variable expansion.
#[macro_export]
macro_rules! root {
    ($($args:tt)*) => {{
        use $crate::__macro_internals::{Dir, DirEntry, Encoding, File, Metadata, include_entry};
        const ENTRY: DirEntry = include_entry!($($args)*);
        ENTRY
    }};
}
