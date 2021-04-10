use trillium::{sequence, Conn};
use trillium_tera::{TeraConnExt, TeraHandler};

fn main() {
    trillium_smol_server::run(sequence![
        TeraHandler::new("**/*.html"),
        |conn: Conn| async move { conn.assign("name", "hi").render("examples/hello.html") }
    ]);
}
