use handlebars::{Handlebars, RenderError};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};
use trillium::{Conn, Handler};
/**
A trillium handler that provides registered templates to
downsequence handlers
*/

#[derive(Default, Clone, Debug)]
pub struct HandlebarsHandler(Arc<RwLock<Handlebars<'static>>>);

impl HandlebarsHandler {
    /// Builds a new trillium Handlebars handler from either a directory
    /// glob string or [`PathBuf`] or a
    /// [`handlebars::Handlebars<'static>`](handlebars::Handlebars)
    /// instance
    ///
    /// ## From a glob
    /// ```
    /// # if cfg!(unix) {
    /// # use std::path::PathBuf;
    /// use trillium_handlebars::{HandlebarsConnExt, HandlebarsHandler};
    /// let handler = (
    ///     HandlebarsHandler::new("**/*.hbs"),
    ///     |mut conn: trillium::Conn| async move {
    ///         conn.assign("name", "handlebars")
    ///             .render("examples/templates/hello.hbs")
    ///     },
    /// );
    ///
    /// use trillium_testing::prelude::*;
    /// assert_ok!(get("/").on(&handler), "hello handlebars!");
    /// # }
    /// ```
    /// ## From a [`handlebars::Handlebars`]
    ///
    /// ```
    /// use trillium_handlebars::{HandlebarsHandler, Handlebars, HandlebarsConnExt};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// // building a Handlebars directly
    /// let mut handlebars = Handlebars::new();
    /// handlebars.register_template_string("greet-user", "Hello {{name}}")?;
    /// let handler = (
    ///     HandlebarsHandler::new(handlebars),
    ///     |mut conn: trillium::Conn| async move {
    ///         conn.assign("name", "handlebars")
    ///             .render("greet-user")
    ///     }
    /// );
    ///
    /// use trillium_testing::prelude::*;
    /// assert_ok!(get("/").on(&handler), "Hello handlebars");
    /// # Ok(()) }
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

impl From<&str> for HandlebarsHandler {
    fn from(pattern: &str) -> Self {
        Self::default().glob(pattern)
    }
}

impl From<Handlebars<'static>> for HandlebarsHandler {
    fn from(ah: Handlebars<'static>) -> Self {
        Self(Arc::new(RwLock::new(ah)))
    }
}

impl From<PathBuf> for HandlebarsHandler {
    fn from(path: PathBuf) -> Self {
        path.to_str().unwrap().into()
    }
}

impl Handler for HandlebarsHandler {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.clone())
    }
}
