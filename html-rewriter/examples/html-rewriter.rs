use trillium_html_rewriter::{
    html::{element, html_content::ContentType, Settings},
    HtmlRewriter,
};
use trillium_proxy::{Proxy, Rustls, TcpStream};

pub fn main() {
    env_logger::init();
    trillium_smol_server::run(trillium::sequence![
        Proxy::<Rustls<TcpStream>>::new("http://neverssl.com"),
        HtmlRewriter::new(|| Settings {
            element_content_handlers: vec![element!("body", |el| {
                el.prepend("<h1>rewritten</h1>", ContentType::Html);
                Ok(())
            })],

            ..Settings::default()
        })
    ]);
}
