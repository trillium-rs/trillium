use trillium_html_rewriter::{
    html::{element, html_content::ContentType, Settings},
    HtmlRewriter,
};

type Proxy = trillium_proxy::Proxy<trillium_smol::TcpConnector>;

pub fn main() {
    env_logger::init();
    trillium_smol::run((
        Proxy::new("http://neverssl.com"),
        HtmlRewriter::new(|| Settings {
            element_content_handlers: vec![element!("body", |el| {
                el.prepend("<h1>rewritten</h1>", ContentType::Html);
                Ok(())
            })],

            ..Settings::default()
        }),
    ));
}
