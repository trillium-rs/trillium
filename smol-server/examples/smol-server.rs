pub fn main() {
    env_logger::init();
    myco_smol_server::run(|conn: myco::Conn| async move { conn.ok("hello") });
}
