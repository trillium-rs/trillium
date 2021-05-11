#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
use futures_lite::io::BufReader;
pub use lol_async::html;
use lol_async::{html::Settings, lol};
use std::str::FromStr;
use trillium::async_trait;
use trillium::http_types::{
    headers::{CONTENT_LENGTH, CONTENT_TYPE},
    mime::Mime,
};
use trillium::{http_types::Body, Conn, Handler};

pub struct HtmlRewriter {
    settings: Box<dyn Fn() -> Settings<'static, 'static> + Send + Sync + 'static>,
}

#[async_trait]
impl Handler for HtmlRewriter {
    async fn run(&self, mut conn: Conn) -> Conn {
        let html = conn
            .headers_mut()
            .get(CONTENT_TYPE)
            .and_then(|c| Mime::from_str(c.as_str()).ok())
            .map(|m| m.subtype() == "html")
            .unwrap_or_default();

        if html && conn.inner().response_body().is_some() {
            let body = conn.inner_mut().take_response_body().unwrap();
            let (fut, reader) = lol(body, (self.settings)());
            async_global_executor::spawn_local(fut).detach();
            conn.headers_mut().remove(CONTENT_LENGTH); // we no longer know the content length, if we ever did
            conn.with_body(Body::from_reader(BufReader::new(reader), None))
        } else {
            conn
        }
    }
}

impl HtmlRewriter {
    pub fn new(f: impl Fn() -> Settings<'static, 'static> + Send + Sync + 'static) -> Self {
        Self {
            settings: Box::new(f)
                as Box<dyn Fn() -> Settings<'static, 'static> + Send + Sync + 'static>,
        }
    }
}
