use handlebars::Handlebars as ActualHandlebars;
use trillium::{async_trait, Conn, Handler};
use serde::Serialize;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
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
impl Handler for Handlebars {
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        conn.with_state(self.clone())
    }
}

trait PrivateConnExt {}

pub trait HandlebarsConnExt {
    fn assign(self, key: impl Into<Cow<'static, str>> + Sized, data: impl Serialize) -> Self;
    fn render_with(self, template: &str, data: &impl Serialize) -> Self;
    fn render(self, template: &str) -> Self;
    fn assigns(&self) -> Option<&Assigns>;
    fn assigns_mut(&mut self) -> &mut Assigns;
}

#[derive(Default, Serialize)]
pub struct Assigns(HashMap<Cow<'static, str>, Value>);

impl Deref for Assigns {
    type Target = HashMap<Cow<'static, str>, Value>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Assigns {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl HandlebarsConnExt for Conn {
    fn render_with(self, template: &str, data: &impl Serialize) -> Self {
        let h: &Handlebars = self
            .state()
            .expect("HandlebarsConnExt::render called without running the handler first");
        let string = h.0.read().unwrap().render(template, data);
        match string {
            Ok(string) => self.ok(string),
            Err(b) => self.status(500).body(b.to_string()),
        }
    }

    fn assign(mut self, key: impl Into<Cow<'static, str>>, data: impl Serialize) -> Self {
        self.assigns_mut().insert(
            key.into(),
            serde_json::to_value(data).expect("could not serialize assigns"),
        );
        self
    }

    fn render(self, template: &str) -> Self {
        let h: &Handlebars = self
            .state()
            .expect("HandlebarsConnExt::render called without running the handler first");

        let string = if let Some(assigns) = self.assigns() {
            h.0.read().unwrap().render(template, assigns)
        } else {
            h.0.read().unwrap().render(template, &json!({}))
        };

        match string {
            Ok(string) => self.ok(string),
            Err(b) => self.status(500).body(b.to_string()),
        }
    }

    fn assigns(&self) -> Option<&Assigns> {
        self.state()
    }

    fn assigns_mut(&mut self) -> &mut Assigns {
        self.state_or_insert_with(Assigns::default)
    }
}
