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
    /// ## From a glob
    /// ```
    /// use trillium_handlebars::{Handlebars, HandlebarsConnExt};
    /// let handler = (
    ///     Handlebars::new("**/*.hbs"),
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
    /// use trillium_handlebars::{Handlebars, handlebars, HandlebarsConnExt};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// // building a handlebars::Handlebars directly
    /// let mut handlebars = handlebars::Handlebars::new();
    /// handlebars.register_template_string("greet-user", "Hello {{name}}")?;
    /// let handler = (
    ///     Handlebars::new(handlebars),
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
