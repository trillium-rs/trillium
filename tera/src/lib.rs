use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
pub use tera::{Context, Tera};
use trillium::http_types::Body;
use trillium::{async_trait, Conn, Handler};

#[derive(Clone)]
pub struct TeraHandler(Arc<Tera>);

impl TeraHandler {
    pub fn new(dir: &str) -> Self {
        TeraHandler(Arc::new(Tera::new(dir).unwrap()))
    }

    pub fn tera(&self) -> &Tera {
        &*self.0
    }
}

#[async_trait]
impl Handler for TeraHandler {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.clone()).with_state(Context::new())
    }
}

pub trait TeraConnExt {
    fn assign(self, key: &str, value: impl Serialize) -> Self;
    fn tera(&self) -> &Tera;
    fn context_mut(&mut self) -> &mut Context;
    fn context(&self) -> &Context;
    fn render(self, template: &str) -> Self;
}

impl TeraConnExt for Conn {
    fn assign(mut self, key: &str, value: impl Serialize) -> Self {
        self.context_mut().insert(key, &value);
        self
    }

    fn tera(&self) -> &Tera {
        self.state::<TeraHandler>()
            .expect("tera must be run after the tera handler")
            .tera()
    }

    fn context_mut(&mut self) -> &mut Context {
        self.state_mut()
            .expect("context_mut must be run after the tera handler")
    }

    fn context(&self) -> &Context {
        self.state()
            .expect("context must be run after the tera handler")
    }

    fn render(self, template_name: &str) -> Self {
        let context = self.context();
        match self.tera().render(template_name, context) {
            Ok(string) => {
                let mut body = Body::from_string(string);

                if let Some(extension) = PathBuf::from(template_name).extension() {
                    if let Some(mime) = mime_db::lookup(extension.to_string_lossy()) {
                        body.set_mime(mime)
                    }
                }

                self.ok(body)
            }

            Err(e) => self.status(500).body(e.to_string()),
        }
    }
}
