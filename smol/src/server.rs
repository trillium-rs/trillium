#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::SmolServer;

#[cfg(not(unix))]
mod tcp;
#[cfg(not(unix))]
pub use tcp::SmolServer;

pub type Config<A> = trillium_server_common::Config<SmolServer, A>;
