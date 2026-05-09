use crate::{
    StaticConnExt,
    fs_shims::{File, fs},
    options::StaticOptions,
    range,
};
use etag::EntityTag;
use relative_path::RelativePath;
use std::path::{Path, PathBuf};
use trillium::{
    Body, Conn, Handler, HeaderValues,
    KnownHeaderName::{
        AcceptEncoding, AcceptRanges, ContentEncoding, ContentRange, ContentType, Etag, IfRange,
        LastModified, Range, Vary,
    },
    Status, conn_unwrap,
};

/// trillium handler to serve static files from the filesystem
#[derive(Debug)]
pub struct StaticFileHandler {
    fs_root: PathBuf,
    index_file: Option<String>,
    root_is_file: bool,
    options: StaticOptions,
    /// (encoding token, filename suffix without the leading dot), in match
    /// priority order. Empty disables precompressed-sidecar serving.
    precompressed: Vec<(String, String)>,
}

#[derive(Debug)]
enum Record {
    /// (path-for-mime-detection, opened file, optional content-encoding token)
    File(PathBuf, File, Option<String>),
    Dir(PathBuf),
}

fn accept_encoding_allows(accept: &str, encoding: &str) -> bool {
    let mut wildcard_ok = false;
    let mut named_ok = None;
    for part in accept.split(',') {
        let mut iter = part.trim().split(';');
        let token = iter.next().unwrap_or("").trim();
        let q = iter
            .find_map(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .and_then(|q| q.parse::<f32>().ok())
            })
            .unwrap_or(1.0);
        if token.eq_ignore_ascii_case(encoding) {
            named_ok = Some(q > 0.0);
        } else if token == "*" && named_ok.is_none() {
            wildcard_ok = q > 0.0;
        }
    }
    named_ok.unwrap_or(wildcard_ok)
}

fn stream_full_body(file: File, len: u64) -> Body {
    #[cfg(feature = "tokio")]
    let file = async_compat::Compat::new(file);
    Body::new_streaming(file, Some(len))
}

async fn seek_take_body(mut file: File, start: u64, len: u64) -> std::io::Result<Body> {
    #[cfg(feature = "tokio")]
    {
        use tokio_crate::io::AsyncSeekExt as _;
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    #[cfg(not(feature = "tokio"))]
    {
        use futures_lite::AsyncSeekExt as _;
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }

    #[cfg(feature = "tokio")]
    let file = async_compat::Compat::new(file);

    use futures_lite::AsyncReadExt as _;
    Ok(Body::new_streaming(file.take(len), Some(len)))
}

fn append_vary_accept_encoding(mut conn: Conn) -> Conn {
    let vary = conn
        .response_headers()
        .get_str(Vary)
        .map(|existing| HeaderValues::from(format!("{existing}, Accept-Encoding")))
        .unwrap_or_else(|| HeaderValues::from("Accept-Encoding"));
    conn.response_headers_mut().insert(Vary, vary);
    conn
}

impl StaticFileHandler {
    async fn resolve_fs_path(&self, url_path: &str) -> Option<PathBuf> {
        let mut file_path = self.fs_root.clone();
        log::trace!(
            "attempting to resolve {} relative to {}",
            url_path,
            file_path.to_str().unwrap()
        );
        for segment in RelativePath::new(url_path) {
            match segment {
                "." => {}
                ".." => {
                    file_path.pop();
                }
                _ => {
                    file_path.push(segment);
                }
            };
        }

        if file_path.starts_with(&self.fs_root) {
            let path_buf = fs::canonicalize(file_path).await.ok();

            #[cfg(feature = "async-std")]
            return path_buf.map(Into::into);
            #[cfg(not(feature = "async-std"))]
            path_buf
        } else {
            None
        }
    }

    async fn pick_precompressed(
        &self,
        asset_path: &Path,
        accept_encoding: Option<&str>,
    ) -> Option<(PathBuf, String)> {
        if self.precompressed.is_empty() {
            return None;
        }
        let accept = accept_encoding?;
        for (encoding, suffix) in &self.precompressed {
            if !accept_encoding_allows(accept, encoding) {
                continue;
            }
            let mut sidecar = asset_path.as_os_str().to_owned();
            sidecar.push(".");
            sidecar.push(suffix);
            let sidecar = PathBuf::from(sidecar);
            if let Ok(metadata) = fs::metadata(&sidecar).await
                && metadata.is_file()
            {
                return Some((sidecar, encoding.clone()));
            }
        }
        None
    }

    async fn resolve(&self, url_path: &str, accept_encoding: Option<&str>) -> Option<Record> {
        let fs_path = self.resolve_fs_path(url_path).await?;
        let metadata = fs::metadata(&fs_path).await.ok()?;
        if metadata.is_dir() {
            log::trace!("resolved {} as dir {}", url_path, fs_path.to_str().unwrap());
            Some(Record::Dir(fs_path))
        } else if metadata.is_file() {
            if let Some((sidecar, encoding)) =
                self.pick_precompressed(&fs_path, accept_encoding).await
                && let Ok(file) = File::open(&sidecar).await
            {
                return Some(Record::File(fs_path, file, Some(encoding)));
            }
            File::open(&fs_path)
                .await
                .ok()
                .map(|file| Record::File(fs_path, file, None))
        } else {
            None
        }
    }

    /// builds a new StaticFileHandler
    ///
    /// If the fs_root is a file instead of a directory, that file will be served at all paths.
    ///
    /// ```
    /// # #[cfg(not(unix))] fn main() {}
    /// # #[cfg(unix)] fn main() {
    /// # use trillium::{Handler, Status};
    /// # trillium_testing::block_on(async {
    /// use trillium_static::{StaticFileHandler, crate_relative_path};
    /// use trillium_testing::TestServer;
    ///
    /// let mut handler = StaticFileHandler::new(crate_relative_path!("examples/files"));
    /// let app = TestServer::new(handler).await;
    ///
    /// app.get("/").await.assert_status(Status::NotFound); // no index file configured
    ///
    /// app.get("/index.html")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("<h1>hello world</h1>\n")
    ///     .assert_header("content-type", "text/html; charset=utf-8");
    /// # }); }
    /// ```
    pub fn new(fs_root: impl AsRef<Path>) -> Self {
        let fs_root = fs_root.as_ref().canonicalize().unwrap();
        Self {
            fs_root,
            index_file: None,
            root_is_file: false,
            options: StaticOptions::default(),
            precompressed: Vec::new(),
        }
    }

    /// Enable serving precompressed sidecar files for the standard set of
    /// content codings: brotli (`.br`), zstd (`.zst`), and gzip (`.gz`), in
    /// that match-priority order.
    ///
    /// For each request whose `Accept-Encoding` allows one of these codings,
    /// the handler looks for a sibling file at `<asset>.<suffix>` and, if
    /// present, serves it with `Content-Encoding` set to the coding token.
    /// The original asset's MIME type is preserved.
    ///
    /// When precompression is enabled, every response from this handler sets
    /// `Vary: Accept-Encoding` — including the uncompressed-original fallback
    /// — so caches do not serve a compressed response to a client that did
    /// not ask for one (or vice versa).
    ///
    /// Equivalent to chaining three calls:
    ///
    /// ```ignore
    /// handler
    ///     .with_precompressed_variant("br", "br")
    ///     .with_precompressed_variant("zstd", "zst")
    ///     .with_precompressed_variant("gzip", "gz")
    /// ```
    ///
    /// To register additional codings or use only a subset, use
    /// [`with_precompressed_variant`](Self::with_precompressed_variant)
    /// directly.
    ///
    /// This composes with `trillium-compression`: when this handler sets
    /// `Content-Encoding`, the compression middleware skips the body and
    /// passes it through unchanged.
    pub fn with_precompressed(self) -> Self {
        self.with_precompressed_variant("br", "br")
            .with_precompressed_variant("zstd", "zst")
            .with_precompressed_variant("gzip", "gz")
    }

    /// Register a precompressed-sidecar variant. Calls are additive and
    /// preserve registration order — earlier registrations win when a client
    /// accepts more than one.
    ///
    /// `encoding` is the [HTTP content-coding token][content-coding] used in
    /// the `Content-Encoding` response header (e.g. `"br"`, `"gzip"`,
    /// `"zstd"`).  `suffix` is the on-disk filename suffix without the
    /// leading dot (e.g. `"br"`, `"gz"`, `"zst"`).
    ///
    /// Most callers want [`with_precompressed`](Self::with_precompressed),
    /// which registers the standard set with conventional suffixes. Use
    /// this method to register a custom coding (for example, a non-standard
    /// suffix from a build pipeline) or to opt into only a subset of the
    /// defaults.
    ///
    /// [content-coding]: https://www.rfc-editor.org/rfc/rfc9110.html#name-content-codings
    pub fn with_precompressed_variant(
        mut self,
        encoding: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        self.precompressed.push((encoding.into(), suffix.into()));
        self
    }

    /// do not set an etag header
    pub fn without_etag_header(mut self) -> Self {
        self.options.etag = false;
        self
    }

    /// do not set last-modified header
    pub fn without_modified_header(mut self) -> Self {
        self.options.modified = false;
        self
    }

    async fn serve_range(
        &self,
        mut conn: Conn,
        file: File,
        source_path: &Path,
        spec: range::RangeSpec,
        if_range: Option<&str>,
    ) -> Conn {
        let metadata = conn_unwrap!(file.metadata().await.ok(), conn);
        let total = metadata.len();

        if self.options.modified
            && let Ok(last_modified) = metadata.modified()
        {
            conn.response_headers_mut()
                .try_insert(LastModified, httpdate::fmt_http_date(last_modified));
        }
        let etag_str = self.options.etag.then(|| {
            let etag = EntityTag::from_file_meta(&metadata).to_string();
            conn.response_headers_mut().try_insert(Etag, etag.clone());
            etag
        });
        conn.response_headers_mut()
            .try_insert(AcceptRanges, "bytes");

        let mime_path = source_path.to_path_buf();
        if let Some(mime) = mime_guess::from_path(&mime_path).first() {
            use mime_guess::mime::{APPLICATION, HTML, JAVASCRIPT, TEXT};
            let is_text = matches!(
                (mime.type_(), mime.subtype()),
                (APPLICATION, JAVASCRIPT) | (TEXT, _) | (_, HTML)
            );
            conn.response_headers_mut().try_insert(
                ContentType,
                if is_text {
                    format!("{mime}; charset=utf-8")
                } else {
                    mime.to_string()
                },
            );
        }

        // If-Range gate
        let last_modified = metadata.modified().ok();
        let honor = if_range
            .is_none_or(|hv| range::if_range_matches(hv, etag_str.as_deref(), last_modified));

        if !honor {
            // Validator mismatch: serve full body 200 from this same file.
            return conn.ok(stream_full_body(file, total));
        }

        match range::resolve(spec, total) {
            Some((start, end)) => {
                let len = end - start + 1;
                match seek_take_body(file, start, len).await {
                    Ok(body) => {
                        conn.response_headers_mut()
                            .insert(ContentRange, format!("bytes {start}-{end}/{total}"));
                        conn.with_status(Status::PartialContent).with_body(body)
                    }
                    Err(_) => conn.with_status(Status::InternalServerError),
                }
            }
            None => {
                conn.response_headers_mut()
                    .insert(ContentRange, format!("bytes */{total}"));
                conn.with_status(Status::RequestedRangeNotSatisfiable)
                    .with_body("")
            }
        }
    }

    /// sets the index file on this StaticFileHandler
    /// ```
    /// # #[cfg(not(unix))] fn main() {}
    /// # #[cfg(unix)] fn main() {
    /// # use trillium::Handler;
    /// # use trillium_testing::TestServer;
    /// # trillium_testing::block_on(async {
    ///
    /// use trillium_static::{StaticFileHandler, crate_relative_path};
    ///
    /// let handler = StaticFileHandler::new(crate_relative_path!("examples/files"))
    ///     .with_index_file("index.html");
    /// let app = TestServer::new(handler).await;
    ///
    /// app.get("/")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body("<h1>hello world</h1>\n")
    ///     .assert_header("content-type", "text/html; charset=utf-8");
    /// # }); }
    /// ```
    pub fn with_index_file(mut self, file: &str) -> Self {
        self.index_file = Some(file.to_string());
        self
    }
}

impl Handler for StaticFileHandler {
    async fn init(&mut self, _info: &mut trillium::Info) {
        self.root_is_file = match self.resolve("/", None).await {
            Some(Record::File(path, _, _)) => {
                log::info!("serving {:?} for all paths", path);
                true
            }

            Some(Record::Dir(dir)) => {
                log::info!("serving files within {:?}", dir);
                false
            }

            None => {
                log::error!(
                    "could not find {:?} on init, continuing anyway",
                    self.fs_root
                );
                false
            }
        };
    }

    async fn run(&self, conn: Conn) -> Conn {
        let accept_encoding = conn
            .request_headers()
            .get_str(AcceptEncoding)
            .map(str::to_owned);
        let range_spec = conn.request_headers().get_str(Range).and_then(range::parse);
        let if_range = conn.request_headers().get_str(IfRange).map(str::to_owned);
        let precompressed_enabled = !self.precompressed.is_empty();

        // When a parsable Range is present, bypass sidecar selection — the
        // range applies to the identity representation.
        let accept_for_resolve = if range_spec.is_some() {
            None
        } else {
            accept_encoding.as_deref()
        };

        let conn = match self.resolve(conn.path(), accept_for_resolve).await {
            Some(Record::File(path, file, encoding)) => {
                if let Some(spec) = range_spec {
                    self.serve_range(conn, file, &path, spec, if_range.as_deref())
                        .await
                } else {
                    let conn = match encoding {
                        Some(enc) => conn.with_response_header(ContentEncoding, enc),
                        None => conn,
                    };
                    let mut conn = conn
                        .send_file_with_options(file, &self.options)
                        .await
                        .with_mime_from_path(path);
                    conn.response_headers_mut()
                        .try_insert(AcceptRanges, "bytes");
                    conn
                }
            }

            Some(Record::Dir(path)) => {
                let index = conn_unwrap!(self.index_file.as_ref(), conn);
                let index_path = path.join(index);

                if let Some(spec) = range_spec {
                    let file = conn_unwrap!(File::open(&index_path).await.ok(), conn);
                    self.serve_range(conn, file, &index_path, spec, if_range.as_deref())
                        .await
                } else {
                    let (open_path, encoding) = match self
                        .pick_precompressed(&index_path, accept_encoding.as_deref())
                        .await
                    {
                        Some((sidecar, encoding)) => (sidecar, Some(encoding)),
                        None => (index_path.clone(), None),
                    };
                    let file = conn_unwrap!(File::open(&open_path).await.ok(), conn);
                    let conn = match encoding {
                        Some(enc) => conn.with_response_header(ContentEncoding, enc),
                        None => conn,
                    };
                    let mut conn = conn
                        .send_file_with_options(file, &self.options)
                        .await
                        .with_mime_from_path(index_path);
                    conn.response_headers_mut()
                        .try_insert(AcceptRanges, "bytes");
                    conn
                }
            }

            None => return conn,
        };

        if precompressed_enabled {
            append_vary_accept_encoding(conn)
        } else {
            conn
        }
    }
}
