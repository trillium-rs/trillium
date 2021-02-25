use myco::{sequence, Conn};
use myco_tera::{TeraConnExt, TeraHandler};

fn main() {
    myco_smol_server::run(sequence![
        TeraHandler::new("**/*.html"),
        |conn: Conn| async move { conn.assign("name", "hi").render("examples/hello.html") }
    ]);
}
