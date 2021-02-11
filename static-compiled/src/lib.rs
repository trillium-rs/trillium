pub use include_dir::include_dir;
use include_dir::{Dir, DirEntry, File};
use myco::http_types::content::ContentType;
use myco::{async_trait, Conn, Handler};

pub enum IndexBehavior {
    None,
    File(&'static str),
}

pub struct StaticCompiled {
    dir: Dir<'static>,
    index_behavior: IndexBehavior,
}

impl StaticCompiled {
    pub fn new(dir: Dir<'static>) -> Self {
        Self {
            dir,
            index_behavior: IndexBehavior::None,
        }
    }

    pub fn with_index_behavior(mut self, index_behavior: IndexBehavior) -> Self {
        self.index_behavior = index_behavior;
        self
    }

    fn serve_file(&self, mut conn: Conn, file: File) -> Conn {
        if let Some(mime) = mime_db::lookup(file.path().to_string_lossy().as_ref()) {
            ContentType::new(mime).apply(conn.headers_mut());
        }
        conn.ok(file.contents())
    }

    fn get_item(&self, path: &str) -> Option<DirEntry> {
        if path == "" {
            Some(DirEntry::Dir(self.dir))
        } else if let Some(dir) = self.dir.get_dir(path) {
            Some(DirEntry::Dir(dir))
        } else if let Some(file) = self.dir.get_file(path) {
            Some(DirEntry::File(file))
        } else {
            None
        }
    }
}

#[async_trait]
impl Handler for StaticCompiled {
    async fn run(&self, conn: myco::Conn) -> myco::Conn {
        match dbg!(self.get_item(conn.path().trim_start_matches('/'))) {
            Some(DirEntry::File(file)) => self.serve_file(conn, file),
            Some(DirEntry::Dir(dir)) => match self.index_behavior {
                IndexBehavior::None => conn,
                IndexBehavior::File(relative_index) => {
                    if let Some(file) = dir.get_file(relative_index) {
                        self.serve_file(conn, file)
                    } else {
                        conn
                    }
                }
            },
            None => conn,
        }
    }
}
