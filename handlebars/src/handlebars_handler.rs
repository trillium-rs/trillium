use handlebars::{Handlebars, RenderError};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};
use trillium::{async_trait, Conn, Handler};
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
    ///
    /// this example uses a pathbuf in order to work
    /// cross-platform. if your code is not run cross-platform, you
    /// can use a &str
    /// ```
    /// # use std::path::PathBuf;
    /// use trillium_handlebars::{HandlebarsHandler, HandlebarsConnExt};
    /// let path: PathBuf = ["**", "*.hbs"].iter().collect();
    /// let handler = (
    ///     HandlebarsHandler::new(path),
    ///     |mut conn: trillium::Conn| async move {
    ///         conn.assign("name", "handlebars")
    ///             .render("examples/templates/hello.hbs")
    ///     }
    /// );
    ///
    /// use trillium_testing::{TestHandler, assert_ok};
    /// let test_handler = TestHandler::new(handler);
    /// assert_ok!(test_handler.get("/"), "hello handlebars!");
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
    /// use trillium_testing::{TestHandler, assert_ok};
    /// let test_handler = TestHandler::new(handler);
    /// assert_ok!(test_handler.get("/"), "Hello handlebars");
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

#[async_trait]
impl Handler for HandlebarsHandler {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.clone())
    }
}
