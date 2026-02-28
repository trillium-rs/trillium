use crate::{
    StaticConnExt,
    fs_shims::{File, fs},
    options::StaticOptions,
};
use std::path::{Path, PathBuf};
use trillium::{Conn, Handler, conn_unwrap};

/// trillium handler to serve static files from the filesystem
#[derive(Debug)]
pub struct StaticFileHandler {
    fs_root: PathBuf,
    index_file: Option<String>,
    root_is_file: bool,
    options: StaticOptions,
}

#[derive(Debug)]
enum Record {
    File(PathBuf, File),
    Dir(PathBuf),
}

impl StaticFileHandler {
    async fn resolve_fs_path(&self, url_path: &str) -> Option<PathBuf> {
        let mut file_path = self.fs_root.clone();
        log::trace!(
            "attempting to resolve {} relative to {}",
            url_path,
            file_path.to_str().unwrap()
        );
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
            fs::canonicalize(file_path).await.ok().map(Into::into)
        } else {
            None
        }
    }

    async fn resolve(&self, url_path: &str) -> Option<Record> {
        let fs_path = self.resolve_fs_path(url_path).await?;
        let metadata = fs::metadata(&fs_path).await.ok()?;
        if metadata.is_dir() {
            log::trace!("resolved {} as dir {}", url_path, fs_path.to_str().unwrap());
            Some(Record::Dir(fs_path))
        } else if metadata.is_file() {
            File::open(&fs_path)
                .await
                .ok()
                .map(|file| Record::File(fs_path, file))
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
    /// # use trillium::Handler;
    /// # trillium_testing::block_on(async {
    /// use trillium_static::{StaticFileHandler, crate_relative_path};
    /// use trillium_testing::prelude::*;
    ///
    /// let mut handler = StaticFileHandler::new(crate_relative_path!("examples/files"));
    /// # handler.init(&mut "testing".into()).await;
    ///
    /// assert_not_handled!(get("/").run_async(&handler).await); // no index file configured
    ///
    /// assert_ok!(
    /// get("/index.html").run_async(&handler).await,
    /// "<h1>hello world</h1>",
    /// "content-type" => "text/html; charset=utf-8"
    /// );
    /// # }); }
    /// ```
    pub fn new(fs_root: impl AsRef<Path>) -> Self {
        let fs_root = fs_root.as_ref().canonicalize().unwrap();
        Self {
            fs_root,
            index_file: None,
            root_is_file: false,
            options: StaticOptions::default(),
        }
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
    /// # trillium_testing::block_on(async {
    ///
    /// use trillium_static::{StaticFileHandler, crate_relative_path};
    ///
    /// let mut handler = StaticFileHandler::new(crate_relative_path!("examples/files"))
    /// .with_index_file("index.html");
    /// # handler.init(&mut "testing".into()).await;
    ///
    /// use trillium_testing::prelude::*;
    /// assert_ok!(
    /// get("/").run_async(&handler).await,
    /// "<h1>hello world</h1>", "content-type" => "text/html; charset=utf-8"
    /// );
    /// # }); }
    /// ```
    pub fn with_index_file(mut self, file: &str) -> Self {
        self.index_file = Some(file.to_string());
        self
    }
}

impl Handler for StaticFileHandler {
    async fn init(&mut self, _info: &mut trillium::Info) {
        self.root_is_file = match self.resolve("/").await {
            Some(Record::File(path, _)) => {
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
        match self.resolve(conn.path()).await {
            Some(Record::File(path, file)) => conn.send_file(file).await.with_mime_from_path(path),

            Some(Record::Dir(path)) => {
                let index = conn_unwrap!(self.index_file.as_ref(), conn);
                let path = path.join(index);
                let file = conn_unwrap!(File::open(path.to_str().unwrap()).await.ok(), conn);
                conn.send_file_with_options(file, &self.options)
                    .await
                    .with_mime_from_path(path)
            }

            _ => conn,
        }
    }
}
