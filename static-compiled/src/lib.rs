pub use include_dir::include_dir;
use include_dir::Dir;
use myco::async_trait;
use myco::http_types::content::ContentType;

pub struct StaticCompiled(Dir<'static>);
impl StaticCompiled {
    pub fn new(dir: Dir<'static>) -> Self {
        Self(dir)
    }
}

#[async_trait]
impl myco::Handler for StaticCompiled {
    async fn run(&self, mut conn: myco::Conn) -> myco::Conn {
        let path = conn.path().trim_start_matches('/');
        if let Some(file) = self.0.get_file(path) {
            if let Some(mime) = mime_db::lookup(file.path().to_string_lossy().as_ref()) {
                ContentType::new(mime).apply(conn.headers_mut());
            }
            conn.ok(file.contents())
        } else {
            conn
        }
    }
}
