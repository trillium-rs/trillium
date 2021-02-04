use myco::{sequence, Conn};
use myco_tera::{TeraConnExt, TeraGrain};

fn main() {
    myco_smol_server::run(
        "127.0.0.1:8081",
        (),
        sequence![TeraGrain::new("**/*.html"), |conn: Conn| async move {
            conn.assign("name", "hi").render("examples/hello.html")
        }],
    );
}
