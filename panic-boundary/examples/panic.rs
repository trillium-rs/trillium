use trillium::Conn;
use trillium_panic_boundary::Unwind;

fn main() {
    trillium_smol::run(Unwind::new(|conn: Conn| async move {
        if conn.path().starts_with("/panic") {
            panic!("PANIC: {} {}", conn.method(), conn.path());
        } else {
            conn.ok("no panic")
        }
    }));
}
