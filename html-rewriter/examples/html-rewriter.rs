use myco_html_rewriter::{
    html::{element, html_content::ContentType, Settings},
    HtmlRewriter,
};
use myco_proxy::{Proxy, Rustls, TcpStream};

pub fn main() {
    env_logger::init();
    myco_smol_server::run(myco::sequence![
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
