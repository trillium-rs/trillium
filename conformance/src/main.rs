//! External-spec conformance runner for trillium.
//!
//! Today this runs [`h2spec`](https://github.com/summerwind/h2spec) against a live trillium
//! HTTP/2 server and diffs the results against a tracked pass-set. Other conformance suites
//! (h3spec, etc.) can be added as additional subcommands.
//!
//! ```text
//! cargo run -p trillium-conformance -- h2spec
//! cargo run -p trillium-conformance -- h2spec --runtime smol --tls rustls
//! cargo run -p trillium-conformance -- all
//! ```

use clap::{Parser, Subcommand, ValueEnum};

mod h2spec;
mod server;

#[derive(Parser)]
#[command(about = "External-spec conformance runner for trillium.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run h2spec against a trillium HTTP/2 server.
    H2spec(h2spec::Args),
    /// Run every suite against every runtime × TLS combination, exit nonzero on any failure.
    All,
}

/// Runtime adapter selection. All three are always linked; selection is runtime-only.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Runtime {
    Tokio,
    Smol,
    AsyncStd,
}

/// TLS / cleartext configuration. `None` is HTTP/2 cleartext (h2c prior-knowledge).
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Tls {
    None,
    Rustls,
}

fn main() -> anyhow::Result<()> {
    env_logger::builder().format_timestamp(None).try_init().ok();
    let cli = Cli::parse();
    match cli.command {
        Command::H2spec(args) => h2spec::run(args),
        Command::All => h2spec::run_all(),
    }
}
