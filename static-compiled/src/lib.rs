pub use include_dir::include_dir;
use include_dir::{Dir, DirEntry, File};
use trillium::http_types::content::ContentType;
use trillium::{async_trait, Conn, Handler};

pub struct StaticCompiled {
    dir: Dir<'static>,
    index_file: Option<&'static str>,
}

impl StaticCompiled {
    pub fn new(dir: Dir<'static>) -> Self {
        Self {
            dir,
            index_file: None,
        }
    }

    pub fn with_index_file(mut self, file: &'static str) -> Self {
        self.index_file = Some(file);
        self
    }

    fn serve_file(&self, mut conn: Conn, file: File) -> Conn {
        if let Some(mime) = mime_db::lookup(file.path().to_string_lossy().as_ref()) {
            conn.headers_mut().apply(ContentType::new(mime));
        }
        conn.ok(file.contents())
    }

    fn get_item(&self, path: &str) -> Option<DirEntry> {
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
impl Handler for StaticCompiled {
    async fn run(&self, conn: Conn) -> Conn {
        match (
            self.get_item(conn.path().trim_start_matches('/')),
            self.index_file,
        ) {
            (None, _) => conn,
            (Some(DirEntry::File(file)), _) => self.serve_file(conn, file),
            (Some(DirEntry::Dir(_)), None) => conn,
            (Some(DirEntry::Dir(dir)), Some(index_file)) => {
                if let Some(file) = dir.get_file(index_file) {
                    self.serve_file(conn, file)
                } else {
                    conn
                }
            }
        }
    }
}
