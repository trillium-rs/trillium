use myco::http_types::Body;
use myco::{async_trait, Conn, Grain};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
pub use tera::{Context, Tera};

#[derive(Clone)]
pub struct TeraGrain(Arc<Tera>);

impl TeraGrain {
    pub fn new(dir: &str) -> Self {
        TeraGrain(Arc::new(Tera::new(dir).unwrap()))
    }

    pub fn tera(&self) -> &Tera {
        &*self.0
    }
}

#[async_trait]
impl Grain for TeraGrain {
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
        self.state::<TeraGrain>()
            .expect("tera must be run after the tera grain")
            .tera()
    }

    fn context_mut(&mut self) -> &mut Context {
        self.state_mut()
            .expect("context_mut must be run after the tera grain")
    }

    fn context(&self) -> &Context {
        self.state()
            .expect("context must be run after the tera grain")
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
