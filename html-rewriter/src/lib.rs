#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
trillium handler that rewrites html using lol-html.

this crate currently requires configuring the runtime,
unfortunately. use one of the `"async-std"`, `"smol"`, or `"tokio"`
features. do not use the default feature, it may change at any time.

```
use trillium_html_rewriter::{
    html::{element, html_content::ContentType, Settings},
    HtmlRewriter,
};

let handler = (
    |conn: trillium::Conn| async move {
        conn.with_header(("content-type", "text/html"))
            .with_status(200)
            .with_body("<html><body><p>body</p></body></html>")
    },
    HtmlRewriter::new(|| Settings {
        element_content_handlers: vec![element!("body", |el| {
            el.prepend("<h1>title</h1>", ContentType::Html);
            Ok(())
        })],
         ..Settings::default()
    }),
);

# async_global_executor::block_on(async move {
use trillium_testing::{methods::*, assert_ok};

let conn = async_global_executor::spawn(async move {
    get("/").run_async(&handler).await
}).await;

assert_ok!(conn, "<html><body><h1>title</h1><p>body</p></body></html>");
# });
*/

use cfg_if::cfg_if;
use futures_lite::io::BufReader;
pub use lol_async::html;
use lol_async::{html::Settings, rewrite};
use std::future::Future;
use std::str::FromStr;
use trillium::async_trait;
use trillium::http_types::{
    headers::{CONTENT_LENGTH, CONTENT_TYPE},
    mime::Mime,
};
use trillium::{http_types::Body, Conn, Handler};

/**
trillium handler for html rewriting
*/
pub struct HtmlRewriter {
    settings: Box<dyn Fn() -> Settings<'static, 'static> + Send + Sync + 'static>,
}

impl std::fmt::Debug for HtmlRewriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HtmlRewriter").finish()
    }
}

fn spawn_local(fut: impl Future + 'static) {
    cfg_if! {
        if #[cfg(feature = "async-std")] {
            async_std_crate::task::spawn_local(fut);
        } else if #[cfg(feature = "smol")] {
            async_global_executor::spawn_local(fut).detach();
        } else if #[cfg(feature = "tokio")] {
            tokio_crate::task::spawn_local(fut);
        } else {
            dbg!("HERE");
            async_global_executor::spawn_local(fut).detach();
        }
    }
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
            let (fut, reader) = rewrite(body, (self.settings)());
            spawn_local(fut);
            conn.headers_mut().remove(CONTENT_LENGTH); // we no longer know the content length, if we ever did
            conn.with_body(Body::from_reader(BufReader::new(reader), None))
        } else {
            conn
        }
    }
}

impl HtmlRewriter {
    /**
    construct a new html rewriter from the provided `fn() ->
    Settings`. See [`lol_html::Settings`] for more information.
     */
    pub fn new(f: impl Fn() -> Settings<'static, 'static> + Send + Sync + 'static) -> Self {
        Self {
            settings: Box::new(f)
                as Box<dyn Fn() -> Settings<'static, 'static> + Send + Sync + 'static>,
        }
    }
}
