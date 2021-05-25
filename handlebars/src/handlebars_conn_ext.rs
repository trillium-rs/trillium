use crate::{Assigns, Handlebars};
use serde::Serialize;
use serde_json::json;
use std::borrow::Cow;
use trillium::Conn;

/**
Extension trait that provides handlebar rendering capabilities to
[`trillium::Conn`]s.
*/
pub trait HandlebarsConnExt {
    /**
    registers an "assigns" value on this Conn for use in a template.
    ```
    todo!()
    ```
    */
    fn assign(self, key: impl Into<Cow<'static, str>> + Sized, data: impl Serialize) -> Self;

    /**
    renders a registered template by name with the provided data as
    assigns. note that this does not use any data accumulated by
    [`HandlebarsConnExt::assign`]

    ```
    todo!()
    ```
    */
    fn render_with(self, template: &str, data: &impl Serialize) -> Self;

    /**
    renders a registered template, passing any accumulated assigns to
    the template

    ```
    todo!()
    ```
     */
    fn render(self, template: &str) -> Self;

    /**
    retrieves a reference to any accumulated assigns on this conn

    ```
    todo!()
    ```
     */
    fn assigns(&self) -> Option<&Assigns>;

    /**
    retrieves a mutable reference to any accumulated assigns on this
    conn
     */
    fn assigns_mut(&mut self) -> &mut Assigns;
}

impl HandlebarsConnExt for Conn {
    fn render_with(self, template: &str, data: &impl Serialize) -> Self {
        let handlebars: &Handlebars = self
            .state()
            .expect("HandlebarsConnExt::render called without running the handler first");

        match handlebars.render(template, data) {
            Ok(string) => self.ok(string),
            Err(b) => self.with_status(500).with_body(b.to_string()),
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
        let handlebars: &Handlebars = self
            .state()
            .expect("HandlebarsConnExt::render called without running the handler first");

        let string = if let Some(assigns) = self.assigns() {
            handlebars.render(template, assigns)
        } else {
            handlebars.render(template, &json!({}))
        };

        match string {
            Ok(string) => self.ok(string),
            Err(b) => self.with_status(500).with_body(b.to_string()),
        }
    }

    fn assigns(&self) -> Option<&Assigns> {
        self.state()
    }

    fn assigns_mut(&mut self) -> &mut Assigns {
        self.mut_state_or_insert_with(Assigns::default)
    }
}
