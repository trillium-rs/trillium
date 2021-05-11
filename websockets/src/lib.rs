#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
mod websocket_connection;
use async_dup::Arc;
use sha1::{Digest, Sha1};
use std::future::Future;
use std::marker::Send;
use trillium::Upgrade;
use trillium::{
    async_trait,
    http_types::{
        headers::{CONNECTION, UPGRADE},
        StatusCode,
    },
    Conn, Handler,
};

pub use async_tungstenite;
pub use async_tungstenite::tungstenite;
pub use tungstenite::{Error, Message};
pub use websocket_connection::WebSocketConnection;

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
pub type Result = std::result::Result<Message, Error>;

#[derive(Debug)]
pub struct WebSocket<Handler> {
    handler: Arc<Handler>,
    protocols: Vec<String>,
}

impl<Handler, Fut> WebSocket<Handler>
where
    Handler: Fn(WebSocketConnection) -> Fut + Sync + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    /// Build a new WebSocket with a handler function that
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

#[async_trait]
impl<H, Fut> Handler for WebSocket<H>
where
    H: Fn(WebSocketConnection) -> Fut + Sync + Send + 'static,
    Fut: Future<Output = ()> + Send + Sync + 'static,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let connection_upgrade = conn
            .headers()
            .contains_ignore_ascii_case(CONNECTION, "upgrade");
        let upgrade_to_websocket = conn
            .headers()
            .contains_ignore_ascii_case(UPGRADE, "websocket");
        let upgrade_requested = connection_upgrade && upgrade_to_websocket;
        log::trace!(
            "{:?} {:?} {:?}",
            connection_upgrade,
            upgrade_to_websocket,
            upgrade_requested
        );

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
        (self.handler)(WebSocketConnection::new(upgrade).await).await
    }
}
