use crate::{
    StaticConnExt,
    fs_shims::{File, fs},
    options::StaticOptions,
};
use relative_path::RelativePath;
use std::path::{Path, PathBuf};
use trillium::{
    Conn, Handler,
    KnownHeaderName::{AcceptEncoding, ContentEncoding, Vary},
    conn_unwrap,
};

/// trillium handler to serve static files from the filesystem
#[derive(Debug)]
pub struct StaticFileHandler {
    fs_root: PathBuf,
    index_file: Option<String>,
    root_is_file: bool,
    options: StaticOptions,
    /// (filename suffix without leading dot, encoding token), in priority order.
    /// Empty disables precompressed-sidecar serving.
    precompressed: Vec<(String, &'static str)>,
}

#[derive(Debug)]
enum Record {
    /// (path-for-mime-detection, opened file, optional content-encoding)
    File(PathBuf, File, Option<&'static str>),
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
    ) -> Option<(PathBuf, &'static str)> {
        if self.precompressed.is_empty() {
            return None;
        }
        let accept = accept_encoding?;
        for (suffix, encoding) in &self.precompressed {
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
                return Some((sidecar, *encoding));
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

    /// Enable precompressed-sidecar serving. For each request whose
    /// `Accept-Encoding` includes a configured encoding token, the handler
    /// looks for a sibling file at `<asset>.<suffix>` and, if present, serves
    /// it with `Content-Encoding: <encoding>` and `Vary: Accept-Encoding`.
    /// The original asset's MIME type is preserved.
    ///
    /// Variants are tried in the order given; the first one that the client
    /// accepts and that exists on disk wins. Pass tokens like:
    /// `&[("br", "br"), ("zst", "zstd"), ("gz", "gzip")]`.
    ///
    /// This composes with `trillium-compression`: when this handler sets
    /// `Content-Encoding`, the compression middleware skips the body and
    /// passes it through unchanged.
    pub fn with_precompressed_sidecars<S>(mut self, variants: &[(S, &'static str)]) -> Self
    where
        S: AsRef<str>,
    {
        self.precompressed = variants
            .iter()
            .map(|(suffix, encoding)| (suffix.as_ref().to_owned(), *encoding))
            .collect();
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
        match self.resolve(conn.path(), accept_encoding.as_deref()).await {
            Some(Record::File(path, file, encoding)) => {
                let conn = match encoding {
                    Some(enc) => conn
                        .with_response_header(ContentEncoding, enc)
                        .with_response_header(Vary, "accept-encoding"),
                    None => conn,
                };
                conn.send_file(file).await.with_mime_from_path(path)
            }

            Some(Record::Dir(path)) => {
                let index = conn_unwrap!(self.index_file.as_ref(), conn);
                let index_path = path.join(index);
                let (open_path, encoding) = match self
                    .pick_precompressed(&index_path, accept_encoding.as_deref())
                    .await
                {
                    Some((sidecar, encoding)) => (sidecar, Some(encoding)),
                    None => (index_path.clone(), None),
                };
                let file = conn_unwrap!(File::open(&open_path).await.ok(), conn);
                let conn = match encoding {
                    Some(enc) => conn
                        .with_response_header(ContentEncoding, enc)
                        .with_response_header(Vary, "accept-encoding"),
                    None => conn,
                };
                conn.send_file_with_options(file, &self.options)
                    .await
                    .with_mime_from_path(index_path)
            }

            _ => conn,
        }
    }
}
