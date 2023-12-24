use trillium::Conn;
use trillium_handlebars::{HandlebarsConnExt, HandlebarsHandler};

fn main() {
    env_logger::init();
    trillium_smol::run((
        HandlebarsHandler::new("./examples/templates/*.hbs"),
        |conn: Conn| async move {
            conn.assign("name", "world")
                .render("examples/templates/hello.hbs")
        },
    ));
}
