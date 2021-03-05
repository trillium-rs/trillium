#![forbid(unsafe_code, future_incompatible)]
#![deny(
    missing_debug_implementations,
    nonstandard_style,
    missing_copy_implementations,
    unused_qualifications
)]

use structopt::StructOpt;

mod cli_options;
#[cfg(unix)]
mod dev_server;
mod root_path;
mod static_cli_options;

pub(crate) use cli_options::*;
#[cfg(unix)]
pub(crate) use dev_server::DevServer;
pub(crate) use root_path::*;
pub(crate) use static_cli_options::*;

pub fn main() {
    Cli::from_args().run()
}
