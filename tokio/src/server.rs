#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::TokioServer;

//#[cfg(not(unix))]
mod tcp;
#[cfg(not(unix))]
pub use tcp::TokioServer;

pub type Config<A> = trillium_server_common::Config<TokioServer, A>;
