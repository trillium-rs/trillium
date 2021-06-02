use trillium::Conn;
use trillium_tera::{TeraConnExt, TeraHandler};

fn main() {
    trillium_smol::run((TeraHandler::new("**/*.html"), |conn: Conn| async move {
        conn.assign("name", "hi").render("examples/hello.html")
    }));
}
