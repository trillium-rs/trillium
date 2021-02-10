use myco::{sequence, Conn};
use myco_handlebars::{Handlebars, HandlebarsConnExt};
use serde_json::json;

fn main() {
    env_logger::init();
    myco_smol_server::run(
        "localhost:8081",
        (),
        sequence![
            Handlebars::new("./examples/templates/*.hbs"),
            |conn: Conn| async move {
                conn.render("examples/templates/hello.hbs", &json!({ "name": "world" }))
            }
        ],
    );
}
