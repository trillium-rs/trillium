use myco::Conn;
use myco_askama::{AskamaConnExt, Template};

#[derive(Template)]
#[template(path = "examples/hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

fn main() {
    myco_smol_server::run("localhost:8081", (), |conn: Conn| async move {
        conn.render(HelloTemplate { name: "world" })
    });
}
