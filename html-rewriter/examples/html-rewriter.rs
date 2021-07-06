use trillium_html_rewriter::{
    html::{element, html_content::ContentType, Settings},
    HtmlRewriter,
};
use trillium_logger::{dev_formatter, formatters::response_header};
use trillium_proxy::Proxy;

pub fn main() {
    env_logger::init();
    trillium_smol::run((
        trillium_logger::Logger::new().with_formatter((
            dev_formatter,
            " ",
            response_header("content-type"),
        )),
        Proxy::new("http://httpbin.org")
            .without_halting()
            .proxy_not_found(),
        HtmlRewriter::new(|| Settings {
            element_content_handlers: vec![element!("h2", |el| {
                el.replace("<h2>rewritten h2</h2>", ContentType::Html);
                Ok(())
            })],

            ..Settings::default()
        }),
    ));
}
