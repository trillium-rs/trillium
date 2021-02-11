use async_fs::{File, ReadDir};
use futures_lite::io::BufReader;
use futures_lite::stream::StreamExt;
use myco::http_types::content::ContentType;
use myco::{async_trait, http_types::Body, Conn, Handler};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Static {
    fs_root: PathBuf,
    url_root: String,
    serve_index: bool,
}

#[derive(Debug)]
enum Record {
    File(PathBuf, File, u64),
    Dir(PathBuf, ReadDir),
}

impl Static {
    async fn resolve_fs_path(&self, url_path: &str) -> Option<PathBuf> {
        if !url_path.starts_with(&self.url_root) {
            return None;
        }

        let url_path = url_path.strip_prefix(&self.url_root).unwrap();

        let mut file_path = self.fs_root.clone();
        for segment in Path::new(url_path) {
            match segment.to_str() {
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
            async_fs::read_dir(&fs_path)
                .await
                .ok()
                .map(|dir| Record::Dir(fs_path, dir))
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

    async fn serve_index(&self, path: PathBuf, dir: ReadDir) -> String {
        let output = dir
            .filter_map(|f| f.ok())
            .map(|f| {
                format!(
                    r#"<li><a href="{}{}">{}</a></li>"#,
                    self.url_root,
                    f.path()
                        .strip_prefix(&self.fs_root)
                        .unwrap()
                        .to_string_lossy(),
                    f.path().file_name().unwrap().to_string_lossy()
                )
            })
            .collect::<Vec<_>>()
            .await
            .join("\n");

        let dotdot = if path.parent().unwrap().starts_with(&self.fs_root) {
            format!(
                r#"<li><a href="{}{}">..</a></li>"#,
                self.url_root,
                path.parent()
                    .unwrap()
                    .strip_prefix(&self.fs_root)
                    .unwrap()
                    .to_string_lossy()
            )
        } else {
            String::from("")
        };

        format!(
            "<html><body><h1>{}</h1><ul>{}{}</ul></body></html>",
            path.strip_prefix(&self.fs_root).unwrap().to_string_lossy(),
            dotdot,
            output
        )
    }

    pub fn new(url_root: &str, fs_root: impl Into<PathBuf>) -> Self {
        Self {
            url_root: url_root.into(),
            fs_root: fs_root.into().canonicalize().unwrap(),
            serve_index: true,
        }
    }
}

#[async_trait]
impl Handler for Static {
    async fn run(&self, mut conn: Conn) -> Conn {
        match self.resolve(conn.path()).await {
            Some(Record::File(path, file, len)) => {
                if let Some(mime) = path.to_str().and_then(mime_db::lookup) {
                    ContentType::new(mime).apply(conn.headers_mut());
                }
                conn.ok(Body::from_reader(BufReader::new(file), Some(len)))
            }

            Some(Record::Dir(path, dir)) if self.serve_index => {
                conn.ok(self.serve_index(path, dir).await)
            }

            _ => conn,
        }
    }
}
