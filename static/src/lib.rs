use async_fs::File;
use futures_lite::io::BufReader;
use trillium::http_types::content::ContentType;
use trillium::{async_trait, conn_ok, http_types::Body, Conn, Handler};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Static {
    fs_root: PathBuf,
    index_file: Option<String>,
}

#[derive(Debug)]
enum Record {
    File(PathBuf, File, u64),
    Dir(PathBuf),
}

impl Static {
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
            return None;
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

    pub fn with_index_file(mut self, file: &str) -> Self {
        self.index_file = Some(file.to_string());
        self
    }

    pub fn new(fs_root: impl Into<PathBuf>) -> Self {
        let fs_root = fs_root.into().canonicalize().unwrap();
        log::info!("serving {:?}", &fs_root);
        Self {
            fs_root,
            index_file: None,
        }
    }

    fn serve_file(mut conn: Conn, path: PathBuf, file: File, len: u64) -> Conn {
        if let Some(mime) = path.to_str().and_then(mime_db::lookup) {
            conn.headers_mut().apply(ContentType::new(mime));
        }

        conn.ok(Body::from_reader(BufReader::new(file), Some(len)))
    }
}

#[async_trait]
impl Handler for Static {
    async fn run(&self, conn: Conn) -> Conn {
        match self.resolve(conn.path()).await {
            Some(Record::File(path, file, len)) => Self::serve_file(conn, path, file, len),

            Some(Record::Dir(path)) => {
                let index = conn_ok!(conn, self.index_file.as_ref().ok_or(""));
                let path = path.join(index);
                let metadata = conn_ok!(conn, async_fs::metadata(&path).await);
                let file = conn_ok!(conn, File::open(path.to_str().unwrap()).await);
                Self::serve_file(conn, path, file, metadata.len())
            }

            _ => conn,
        }
    }
}

#[macro_export]
macro_rules! relative_path {
    ($path:literal) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)
    };
}
