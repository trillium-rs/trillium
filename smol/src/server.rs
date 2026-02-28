#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::SmolServer;

mod tcp;

#[cfg(not(unix))]
pub use tcp::SmolTcpServer as SmolServer;

pub type Config<A> = trillium_server_common::Config<SmolServer, A>;
