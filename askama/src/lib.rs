pub use askama;
pub use askama::Template;

use trillium::http_types::Body;

pub trait AskamaConnExt {
    fn render(self, template: impl Template) -> Self;
}

impl AskamaConnExt for trillium::Conn {
    fn render(self, template: impl Template) -> Self {
        let text = template.render().unwrap();
        let mut body = Body::from_string(text);
        if let Some(extension) = template.extension() {
            if let Some(mime) = mime_db::lookup(extension) {
                body.set_mime(mime);
            }
        }

        self.ok(body)
    }
}
