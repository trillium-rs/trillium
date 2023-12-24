#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::AsyncStdServer;

#[cfg(not(unix))]
mod tcp;
#[cfg(not(unix))]
pub use tcp::AsyncStdServer;

pub type Config<A> = trillium_server_common::Config<AsyncStdServer, A>;
