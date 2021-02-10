use handlebars::Handlebars as ActualHandlebars;
use myco::{async_trait, Conn, Grain};
// use std::ops::{Deref, DerefMut};
// use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;

#[derive(Default, Clone)]
pub struct Handlebars(Arc<RwLock<ActualHandlebars<'static>>>);

impl Handlebars {
    pub fn new(source: impl Into<Self>) -> Self {
        source.into()
    }

    fn glob(&self, s: &str) {
        let mut h = self.0.write().unwrap();
        for file in glob::glob(s).unwrap().filter_map(Result::ok) {
            log::debug!("registered template {:?}", &file);
            h.register_template_file(file.clone().to_string_lossy().as_ref(), file)
                .unwrap();
        }
    }
}

impl From<&'static str> for Handlebars {
    fn from(source: &'static str) -> Self {
        let s = Self::default();
        s.glob(source);
        s
    }
}

impl From<ActualHandlebars<'static>> for Handlebars {
    fn from(ah: ActualHandlebars<'static>) -> Self {
        Self(Arc::new(RwLock::new(ah)))
    }
}

#[async_trait]
impl Grain for Handlebars {
    async fn run(&self, conn: myco::Conn) -> myco::Conn {
        conn.with_state(self.clone())
    }
}

#[async_trait]
pub trait HandlebarsConnExt {
    fn render(self, template: &str, data: &impl serde::Serialize) -> Self;
}

#[async_trait]
impl HandlebarsConnExt for Conn {
    fn render(self, name: &str, data: &impl serde::Serialize) -> Self {
        let h: &Handlebars = self
            .state()
            .expect("HandlebarsConnExt::render called without running the handler first");
        let string = h.0.read().unwrap().render(name, data);
        match string {
            Ok(string) => self.ok(string),
            Err(b) => self.status(500).body(b.to_string()),
        }
    }
}
