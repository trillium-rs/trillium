use std::pin::Pin;

use async_tungstenite::tungstenite::protocol::Role;
use async_tungstenite::{tungstenite::Message, WebSocketStream};
pub use futures_util::stream::Stream;
use futures_util::SinkExt;
use myco::http_types::{headers::Headers, Extensions, Method};
use myco::{BoxedTransport, Upgrade};
use std::task::{Context, Poll};
use stopper::{Stopper, StreamStopper};

#[derive(Debug)]
pub struct WebSocketConnection {
    request_headers: Headers,
    path: String,
    method: Method,
    state: Extensions,
    stopper: Stopper,
    wss: StreamStopper<WebSocketStream<BoxedTransport>>,
}

impl WebSocketConnection {
    pub async fn send_string(&mut self, s: String) {
        self.wss.send(Message::Text(s)).await.ok();
    }

    pub async fn send_bytes(&mut self, bytes: Vec<u8>) {
        self.wss.send(Message::Binary(bytes)).await.ok();
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
            stopper,
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
            wss: stopper.stop_stream(wss),
            stopper,
        }
    }

    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    pub async fn close(&mut self) {
        self.wss.close(None).await.ok();
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

    pub fn take_state<T: 'static>(&mut self) -> Option<T> {
        self.state.remove()
    }
}

impl Stream for WebSocketConnection {
    type Item = crate::Result;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.wss).poll_next(cx)
    }
}
