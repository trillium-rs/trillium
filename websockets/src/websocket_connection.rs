use std::pin::Pin;

use async_tungstenite::tungstenite::protocol::Role;
use async_tungstenite::{tungstenite::Message, WebSocketStream};
pub use futures_util::stream::Stream;
use futures_util::SinkExt;
use myco::http_types::{headers::Headers, Extensions, Method};
use myco::{BoxedTransport, Upgrade};
use std::task::{Context, Poll};

#[derive(Debug)]
pub struct WebSocketConnection {
    request_headers: Headers,
    path: String,
    method: Method,
    state: Extensions,
    wss: WebSocketStream<BoxedTransport>,
}

impl WebSocketConnection {
    pub async fn send_string(&mut self, s: String) {
        self.wss.send(Message::Text(s)).await.unwrap();
    }

    pub async fn send_bytes(&mut self, bytes: Vec<u8>) {
        self.wss.send(Message::Binary(bytes)).await.unwrap();
    }

    pub async fn send_json(&mut self, json: &impl serde::Serialize) -> serde_json::Result<()> {
        self.send_string(serde_json::to_string(json)?).await;
        Ok(())
    }

    pub(crate) async fn new(upgrade: Upgrade) -> Self {
        let Upgrade {
            request_headers,
            path,
            method,
            state,
            buffer,
            rw,
        } = upgrade;

        let wss = if let Some(vec) = buffer {
            WebSocketStream::from_partially_read(rw, vec, Role::Server, None).await
        } else {
            WebSocketStream::from_raw_socket(rw, Role::Server, None).await
        };

        Self {
            request_headers,
            path,
            method,
            state,
            wss,
        }
    }

    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn method(&self) -> &Method {
        &self.method
    }
    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.state.get()
    }
}

impl Stream for WebSocketConnection {
    type Item = crate::Result;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.wss).poll_next(cx)
    }
}
