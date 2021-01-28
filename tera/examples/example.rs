use myco::{Conn, Sequence};
use myco_tera::{TeraConnExt, TeraGrain};

fn main() {
    let grain = Sequence::new()
        .and(TeraGrain::new("**/*.html"))
        .and(|conn: Conn| async move { conn.assign("name", "hi").render("examples/hello.html") });

    myco_smol_server::Server::new("127.0.0.1:8081", grain)
        .unwrap()
        .run();
}
