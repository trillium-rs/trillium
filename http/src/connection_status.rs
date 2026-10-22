use crate::{Conn, Upgrade};
/// This represents the next state after a response on a conn transport.
#[derive(Debug)]
pub enum ConnectionStatus<Transport> {
    /// The transport has been closed, either by the client or by us
    Close,
    /// Another `Conn` request has been sent on the same transport and
    /// is ready to respond to. This can occur any number of times and
    /// should be handled in a loop.
    Conn(Conn<Transport>),
    /// An http upgrade has been negotiated. This is always a terminal
    /// state for a given connection.
    Upgrade(Upgrade<Transport>),
}
