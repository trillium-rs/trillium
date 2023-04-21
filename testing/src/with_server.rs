use crate::{block_on, ServerConnector};
use std::{error::Error, future::Future};
use trillium::Handler;
use trillium_http::transport::BoxedTransport;

cfg_if::cfg_if! {
    if #[cfg(feature = "smol")] {
        mod smol;
        pub use smol::with_server;
    } else if #[cfg(feature = "async-std")] {
        mod async_std;
        pub use async_std::with_server;
    } else if #[cfg(feature = "tokio")] {
        mod tokio;
        pub use tokio::with_server;
   } else {
        ///
        pub fn with_server<H, Fun, Fut>(_handler: H, _tests: Fun)
        where
            H: Handler,
            Fun: FnOnce(crate::Url) -> Fut,
            Fut: Future<Output = Result<(), Box<dyn Error>>>,
        {
            panic!("with_server requires a runtime to be selected")
        }
    }
}

/// open an in-memory connection to this handler and call an async
/// function with an open BoxedTransport
pub fn with_transport<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: Handler,
    Fun: FnOnce(BoxedTransport) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    block_on(async move {
        let transport = ServerConnector::new(handler).connect(false).await;
        tests(BoxedTransport::new(transport));
    });
}
