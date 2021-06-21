use trillium::{Conn, State};
use trillium_logger::{apache_combined, Logger, Target};

#[derive(Clone, Copy)]
struct User(&'static str);

impl User {
    pub fn name(&self) -> &'static str {
        &self.0
    }
}

fn user_id(conn: &Conn, _color: bool) -> &'static str {
    conn.state::<User>().map(User::name).unwrap_or("-")
}

pub fn main() {
    trillium_smol::run((
        State::new(User("jacob")),
        Logger::new()
            .with_formatter(apache_combined("-", user_id))
            .with_target(Target::Stdout),
        "ok",
    ));
}
