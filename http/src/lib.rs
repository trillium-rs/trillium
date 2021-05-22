#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
/*!
This crate provides the http 1.x implementation for trillium.

# Example

This is an elaborate example that demonstrates some of trillium_http's
capabilities.  Please note that trillium itself provides a much more
usable interface on top of trillium_http, at very little cost.

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
    let server = smol::spawn(async move {
        let mut incoming = server_stopper.stop_stream(listener.incoming());

        while let Some(Ok(stream)) = incoming.next().await {
            let stopper = server_stopper.clone();
            smol::spawn(async move {
                Conn::map(stream, stopper, |mut conn: Conn<TcpStream>| async move {
                    conn.set_response_body("hello world");
                    conn.set_status(200);
                    conn
                })
                .await
            })
            .detach()
        }

        Result::Ok(())
    });

    // this example uses the trillium client
    // please note that this api is still especially unstable.
    // any other http client would work here too
    let url = format!("http://localhost:{}/", port);
    let mut client_conn = trillium_client::Conn::<TcpStream>::get(&*url)
        .execute()
        .await?;

    assert_eq!(client_conn.status().unwrap(), 200);
    assert_eq!(
        client_conn.response_body().read_string().await?,
        "hello world"
    );

    stopper.stop(); // stop the server after one request
    server.await?; // wait for the server to shut down
#        Result::Ok(()) }) }
```
*/

mod body_encoder;
#[cfg(feature = "unstable")]
pub use body_encoder::BodyEncoder;

mod chunked_encoder;
#[cfg(feature = "unstable")]
pub use chunked_encoder::ChunkedEncoder;

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

pub use http_types;

pub use stopper::Stopper;

mod mut_cow;
pub(crate) use mut_cow::MutCow;

mod util;

/// Types to represent the bidirectional data stream over which the
/// HTTP protocol is communicated
pub mod transport;
