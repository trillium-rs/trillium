#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
# A websocket trillium handler

```
use trillium_websockets::{Message, WebSocket};
# use futures_util::{SinkExt, StreamExt};
# use async_net::TcpStream;

let handler = WebSocket::new(|mut websocket| async move {
    let path = websocket.path().to_owned();
    while let Some(Ok(Message::Text(input))) = websocket.next().await {
        websocket
            .send_string(format!("received your message: {} at path {}", &input, path))
            .await;
    }
});

trillium_testing::with_server(handler, |url| async move {
    let socket = TcpStream::connect(&url.socket_addrs(|| None)?[..]).await?;
    let (mut client, _) = async_tungstenite::client_async("ws://localhost/some/route", socket).await?;

    client.send(Message::text("hello")).await?;
    let received_message = client.next().await.unwrap()?.into_text()?;
    assert_eq!("received your message: hello at path /some/route", received_message);

    client.send(Message::text("hey")).await?;
    let received_message = client.next().await.unwrap()?.into_text()?;
    assert_eq!("received your message: hey at path /some/route", received_message);

    Ok(())
});
```
*/

mod websocket_connection;
use async_dup::Arc;
use sha1::{Digest, Sha1};
use std::{future::Future, marker::Send};
use trillium::{
    async_trait,
    http_types::{
        headers::{Headers, CONNECTION, UPGRADE},
        StatusCode,
    },
    Conn, Handler, Upgrade,
};

pub use async_tungstenite;
pub use async_tungstenite::tungstenite;
pub use tungstenite::{Error, Message};
pub use websocket_connection::WebSocketConn;

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// a Result type for websocket messages
pub type Result = std::result::Result<Message, Error>;

/**
The trillium handler.
See crate-level docs for example usage.
*/
#[derive(Debug)]
pub struct WebSocket<Handler> {
    handler: Arc<Handler>,
    protocols: Vec<String>,
}

impl<Handler, Fut> WebSocket<Handler>
where
    Handler: Fn(WebSocketConn) -> Fut + Sync + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    /// Build a new WebSocket with an async handler function that
    /// receives a [`WebSocketConn`]
    pub fn new(handler: Handler) -> Self {
        Self {
            handler: Arc::new(handler),
            protocols: Default::default(),
        }
    }

    /// `protocols` is a sequence of known protocols. On successful handshake,
    /// the returned response headers contain the first protocol in this list
    /// which the server also knows.
    pub fn with_protocols(self, protocols: &[&str]) -> Self {
        Self {
            protocols: protocols.iter().map(ToString::to_string).collect(),
            ..self
        }
    }
}

struct IsWebsocket;

fn connection_is_upgrade(headers: &Headers) -> bool {
    headers
        .get(CONNECTION)
        .map(|connection| {
            connection
                .as_str()
                .split(',')
                .any(|c| c.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::connection_is_upgrade;
    use trillium::http_types::headers::Headers;

    #[test]
    fn test_connection_is_upgrade() {
        let mut headers = Headers::new();

        assert!(!connection_is_upgrade(&headers));

        headers.insert("connection", "keep-alive, Upgrade");
        assert!(connection_is_upgrade(&headers));

        headers.insert("connection", "upgrade");
        assert!(connection_is_upgrade(&headers));

        headers.insert("connection", "UPgrAde");
        assert!(connection_is_upgrade(&headers));

        headers.insert("connection", "UPgrAde, keep-alive");
        assert!(connection_is_upgrade(&headers));

        headers.insert("connection", "keep-alive");
        assert!(!connection_is_upgrade(&headers));
    }
}

#[async_trait]
impl<H, Fut> Handler for WebSocket<H>
where
    H: Fn(WebSocketConn) -> Fut + Sync + Send + 'static,
    Fut: Future<Output = ()> + Send + Sync + 'static,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let connection_upgrade = connection_is_upgrade(conn.headers());
        let upgrade_to_websocket = conn
            .headers()
            .contains_ignore_ascii_case(UPGRADE, "websocket");
        let upgrade_requested = connection_upgrade && upgrade_to_websocket;

        if !upgrade_requested {
            return conn;
        }

        let header = match conn.headers().get("Sec-Websocket-Key") {
            Some(h) => h.as_str(),
            None => return conn.with_status(StatusCode::BadRequest),
        };

        let protocol = conn
            .headers()
            .get("Sec-Websocket-Protocol")
            .and_then(|value| {
                value
                    .as_str()
                    .split(',')
                    .map(str::trim)
                    .find(|req_p| self.protocols.iter().any(|p| p == req_p))
                    .map(|s| s.to_owned())
            });

        let hash = Sha1::new().chain(header).chain(WEBSOCKET_GUID).finalize();

        let headers = conn.headers_mut();
        headers.insert(UPGRADE, "websocket");
        headers.insert(CONNECTION, "Upgrade");
        headers.insert("Sec-Websocket-Accept", base64::encode(&hash[..]));
        headers.insert("Sec-Websocket-Version", "13");

        if let Some(protocol) = protocol {
            headers.insert("Sec-Websocket-Protocol", protocol);
        }

        conn.halt()
            .with_state(IsWebsocket)
            .with_status(StatusCode::SwitchingProtocols)
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state().get::<IsWebsocket>().is_some()
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        (self.handler)(WebSocketConn::new(upgrade).await).await
    }
}
