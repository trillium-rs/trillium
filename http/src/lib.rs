#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs, clippy::pedantic, clippy::perf, clippy::cargo)]
#![allow(clippy::must_use_candidate, clippy::module_name_repetitions)]
/*!
This crate provides the http 1.x implementation for Trillium.

## Stability

As this is primarily intended for internal use by the [Trillium
crate](https://docs.trillium.rs/trillium), the api is likely to be
less stable than that of the higher level abstractions in Trillium.

## Example

This is an elaborate example that demonstrates some of `trillium_http`'s
capabilities.  Please note that trillium itself provides a much more
usable interface on top of `trillium_http`, at very little cost.

```
# fn main() -> trillium_http::Result<()> {    smol::block_on(async {
use async_net::{TcpListener, TcpStream};
use futures_lite::StreamExt;
use stopper::Stopper;
use trillium_http::{Conn, Result};

let stopper = Stopper::new();
let listener = TcpListener::bind(("localhost", 0)).await?;
let port = listener.local_addr()?.port();

let server_stopper = stopper.clone();
let server_handle = smol::spawn(async move {
    let mut incoming = server_stopper.stop_stream(listener.incoming());

    while let Some(Ok(stream)) = incoming.next().await {
        let stopper = server_stopper.clone();
        smol::spawn(Conn::map(stream, stopper, |mut conn: Conn<TcpStream>| async move {
            conn.set_response_body("hello world");
            conn.set_status(200);
            conn
         })).detach()
    }

    Result::Ok(())
});

// this example uses the trillium client
// please note that this api is still especially unstable.
// any other http client would work here too
let url = format!("http://localhost:{}/", port);
type ClientConn<'a> = trillium_client::Conn<'a, trillium_smol::TcpConnector>;
let mut client_conn = ClientConn::get(&*url).await?;

assert_eq!(client_conn.status().unwrap(), 200);
assert_eq!(client_conn.response_headers().get_str("content-length"), Some("11"));
assert_eq!(
    client_conn.response_body().read_string().await?,
    "hello world"
);

stopper.stop(); // stop the server after one request
server_handle.await?; // wait for the server to shut down
#        Result::Ok(()) }) }
```
*/

mod received_body;
pub use received_body::ReceivedBody;

#[cfg(feature = "unstable")]
pub use received_body::ReceivedBodyState;

mod error;
pub use error::{Error, Result};

mod conn;
pub use conn::Conn;

mod connection_status;
pub use connection_status::ConnectionStatus;

mod synthetic;
pub use synthetic::Synthetic;

mod upgrade;
pub use upgrade::Upgrade;

pub use stopper::Stopper;

mod mut_cow;
pub(crate) use mut_cow::MutCow;

mod util;

mod body;
pub use body::Body;

mod state_set;
pub use state_set::StateSet;

mod headers;
pub use headers::Headers;

mod header_name;
pub use header_name::{HeaderName, KnownHeaderName};

mod header_values;
pub use header_values::HeaderValues;

mod header_value;
pub use header_value::HeaderValue;

mod status;
pub use status::Status;

mod method;
pub use method::Method;

mod version;
pub use version::Version;

/// Types to represent the bidirectional data stream over which the
/// HTTP protocol is communicated
pub mod transport;

/// A pre-rendered http response to send when the server is at capacity.
pub const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r
Connection: close\r
Content-Length: 0\r
Retry-After: 60\r
\r\n";

#[cfg(feature = "http-compat")]
mod http_compat;
