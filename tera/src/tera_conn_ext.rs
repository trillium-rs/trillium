use crate::TeraHandler;
use serde::Serialize;
use std::path::PathBuf;
use tera::{Context, Tera};
use trillium::{http_types::Body, Conn};

/**
Extends trillium::Conn with tera template-rendering functionality.
*/
pub trait TeraConnExt {
    /// Adds a key-value pair to the assigns [`Context`], where the key is
    /// a &str and the value is any [`Serialize`] type.
    fn assign(self, key: &str, value: impl Serialize) -> Self;

    /// Uses the accumulated assigns context to render the template by
    /// registered name to the conn body and return the conn. Halts
    /// and sets a 200 status on successful render. Must be run
    /// downsequence of the [`TeraHandler`], and will panic if the
    /// TeraHandler has not already been called.
    fn render(self, template: &str) -> Self;

    /// Retrieves a reference to the [`Tera`] instance. Must be called
    /// downsequence of the [`TeraHandler`], and will panic if the
    /// TeraHandler has not already been called.
    fn tera(&self) -> &Tera;

    /// retrieves a reference to the tera assigns context. must be run
    /// downsequence of the [`TeraHandler`], and will panic if the
    /// TeraHandler has not already been called.
    fn context_mut(&mut self) -> &mut Context;

    /// Retrieves a reference to the tera assigns context. Must be run
    /// downsequence of the [`TeraHandler`], and will panic if the
    /// TeraHandler has not already been called.
    fn context(&self) -> &Context;
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

            Err(e) => {
                log::error!("{:?}", &e);
                self.with_status(500).with_body(e.to_string())
            }
        }
    }
}
