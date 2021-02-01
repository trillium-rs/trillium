#![forbid(unsafe_code)]

mod body_encoder;
mod chunked_encoder;
mod request_body;

pub use chunked_encoder::ChunkedEncoder;
pub use request_body::RequestBody;

mod error;
pub use error::{Error, Result};

mod conn;
pub use conn::{Conn, ConnectionStatus};

mod synthetic;
pub use synthetic::Synthetic;

mod upgrade;
pub use upgrade::Upgrade;
