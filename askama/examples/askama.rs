use trillium::Conn;
use trillium_askama::{AskamaConnExt, Template};

#[derive(Template)]
#[template(path = "examples/hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

fn main() {
    trillium_smol_server::run(
        |conn: Conn| async move { conn.render(HelloTemplate { name: "world" }) },
    );
}
