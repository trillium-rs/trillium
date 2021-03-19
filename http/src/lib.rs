#![forbid(unsafe_code)]

mod body_encoder;
pub use body_encoder::BodyEncoder;

mod chunked_encoder;
pub use chunked_encoder::ChunkedEncoder;

mod received_body;
pub use received_body::{ReceivedBody, ReceivedBodyState};

mod error;
pub use error::{Error, Result};

mod conn;
pub use conn::{Conn, ConnectionStatus};

mod synthetic;
pub use synthetic::Synthetic;

mod upgrade;
pub use upgrade::Upgrade;

pub use http_types;

pub use stopper::Stopper;

mod mut_cow;
pub(crate) use mut_cow::MutCow;

pub mod util;
