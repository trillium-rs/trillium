use crate::TestTransport;
use async_channel::{Receiver, Sender};
use dashmap::DashMap;
use once_cell::sync::Lazy;

type Servers = Lazy<DashMap<(String, u16), (Sender<TestTransport>, Receiver<TestTransport>)>>;
static SERVERS: Servers = Lazy::new(Default::default);

mod runtime;
pub use runtime::RuntimelessRuntime;

mod server;
pub use server::RuntimelessServer;

mod client;
pub use client::RuntimelessClientConfig;
