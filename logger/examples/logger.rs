use trillium::{Conn, State};
use trillium_logger::{apache_combined, Logger, Target};

#[derive(Clone, Copy)]
struct User(&'static str);

impl User {
    pub fn name(&self) -> &'static str {
        &self.0
    }
}

pub fn main() {
    trillium_smol::run((
        State::new(User("jacob")),
        Logger::new()
            .with_formatter(apache_combined("-", |conn: &Conn, _color| {
                conn.state::<User>().map(User::name).unwrap_or("-")
            }))
            .with_target(Target::Stdout),
        "ok",
    ));
}
