use std::{path::PathBuf, sync::Arc};
use tera::{Context, Tera};
use trillium::{Conn, Handler};

/**

*/
#[derive(Clone, Debug)]
pub struct TeraHandler(Arc<Tera>);

impl From<PathBuf> for TeraHandler {
    fn from(dir: PathBuf) -> Self {
        dir.to_str().unwrap().into()
    }
}

impl From<&str> for TeraHandler {
    fn from(dir: &str) -> Self {
        Tera::new(dir).unwrap().into()
    }
}

impl From<&String> for TeraHandler {
    fn from(dir: &String) -> Self {
        (**dir).into()
    }
}

impl From<String> for TeraHandler {
    fn from(dir: String) -> Self {
        dir.into()
    }
}

impl From<Tera> for TeraHandler {
    fn from(tera: Tera) -> Self {
        Self(Arc::new(tera))
    }
}

impl From<&[&str]> for TeraHandler {
    fn from(dir_parts: &[&str]) -> Self {
        dir_parts.iter().collect::<PathBuf>().into()
    }
}

impl TeraHandler {
    /// Construct a new TeraHandler from either a `&str` or PathBuf that represents
    /// a directory glob containing templates, or from a
    /// [`tera::Tera`] instance
    /// ```
    /// # fn main() -> tera::Result<()> {
    /// use std::{iter::FromIterator, path::PathBuf};
    /// use trillium_tera::TeraHandler;
    ///
    /// let handler = TeraHandler::new(PathBuf::from_iter([".", "examples", "**", "*.html"]));
    ///
    /// // or
    ///
    /// let handler = TeraHandler::new("examples/*.html");
    ///
    /// // or
    ///
    /// let mut tera = trillium_tera::Tera::default();
    /// tera.add_raw_template("hello.html", "hello {{name}}")?;
    /// let handler = TeraHandler::new(tera);
    /// # Ok(()) }
    /// ```
    pub fn new(tera: impl Into<Self>) -> Self {
        tera.into()
    }

    pub(crate) fn tera(&self) -> &Tera {
        &self.0
    }
}

impl Handler for TeraHandler {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.clone()).with_state(Context::new())
    }
}
