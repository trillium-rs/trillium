use trillium::Conn;
use trillium_handlebars::{Handlebars, HandlebarsConnExt};

fn main() {
    env_logger::init();
    trillium_smol_server::run((
        Handlebars::new("./examples/templates/*.hbs"),
        |conn: Conn| async move {
            conn.assign("name", "world")
                .render("examples/templates/hello.hbs")
        },
    ));
}
