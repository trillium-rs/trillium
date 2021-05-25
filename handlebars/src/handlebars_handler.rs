use handlebars::{Handlebars as ActualHandlebars, RenderError};
use serde::Serialize;
use std::sync::{Arc, RwLock};
use trillium::{async_trait, Conn, Handler};
/**
A trillium handler that provides registered templates to
downsequence handlers
*/

#[derive(Default, Clone, Debug)]
pub struct Handlebars(Arc<RwLock<ActualHandlebars<'static>>>);

impl Handlebars {
    /// Builds a new trillium Handlebars handler from either a directory
    /// glob string or a
    /// [`handlebars::Handlebars<'static>`](handlebars::Handlebars)
    /// instance
    ///
    /// ```
    /// trillium_handlebars::Handlebars::new(trillium_handlebars::handlebars::Handlebars::default());
    /// ```
    ///
    /// ```
    /// trillium_handlebars::Handlebars::new("**/*.hbs");
    /// ```
    pub fn new(source: impl Into<Self>) -> Self {
        source.into()
    }

    pub(crate) fn render(
        &self,
        template: &str,
        data: &impl Serialize,
    ) -> Result<String, RenderError> {
        self.0.read().unwrap().render(template, data)
    }

    fn glob(self, pattern: &str) -> Self {
        {
            let mut handlebars = self.0.write().unwrap();
            for file in glob::glob(pattern).unwrap().filter_map(Result::ok) {
                log::debug!("registered template {:?}", &file);
                handlebars
                    .register_template_file(file.clone().to_string_lossy().as_ref(), file)
                    .unwrap();
            }
        }

        self
    }
}

impl From<&'static str> for Handlebars {
    fn from(pattern: &'static str) -> Self {
        Self::default().glob(pattern)
    }
}

impl From<ActualHandlebars<'static>> for Handlebars {
    fn from(ah: ActualHandlebars<'static>) -> Self {
        Self(Arc::new(RwLock::new(ah)))
    }
}

#[async_trait]
impl Handler for Handlebars {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.clone())
    }
}
