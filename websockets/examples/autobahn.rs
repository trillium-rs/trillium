//! Echo server for the [Autobahn testsuite](https://github.com/crossbario/autobahn-testsuite).
//!
//! ```sh
//! HOST=0.0.0.0 PORT=9001 cargo run -p trillium-websockets --example autobahn
//! docker run --rm -v "$PWD/autobahn:/config" -v "$PWD/autobahn/reports:/reports" \
//!     crossbario/autobahn-testsuite wstest -m fuzzingclient -s /config/fuzzingclient.json
//! ```
//!
//! with an `autobahn/fuzzingclient.json` along the lines of
//!
//! ```json
//! {
//!     "outdir": "/reports/servers",
//!     "servers": [{ "agent": "trillium-websockets", "url": "ws://host.docker.internal:9001" }],
//!     "cases": ["*"],
//!     "exclude-cases": ["12.*", "13.*"]
//! }
//! ```
//!
//! Cases 12 and 13 cover permessage-deflate, which trillium-websockets does not negotiate.

use futures_lite::StreamExt;
use trillium_websockets::{WebSocket, WebSocketConn};

async fn echo(mut conn: WebSocketConn) {
    while let Some(Ok(message)) = conn.next().await {
        if (message.is_text() || message.is_binary()) && conn.send(message).await.is_err() {
            break;
        }
    }
}

fn main() {
    trillium_smol::run(WebSocket::new(echo).allow_any_origin());
}
