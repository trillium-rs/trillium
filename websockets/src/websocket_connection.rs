use crate::Result;
use async_tungstenite::{
    tungstenite::{protocol::Role, Message},
    WebSocketStream,
};
use futures_util::{stream::Stream, SinkExt};
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use stopper::{Stopper, StreamStopper};
use trillium::{
    http_types::{headers::Headers, Extensions, Method},
    Upgrade,
};
use trillium_http::transport::BoxedTransport;

type Wss = StreamStopper<WebSocketStream<BoxedTransport>>;
type SpawnFn = Box<dyn Fn(Pin<Box<dyn Future<Output = ()> + Send>>) + Send>;

/**
A struct that represents an specific websocket connection.

This can be thought of as a combination of a [`async_tungstenite::WebSocketStream`] and a
[`trillium::Conn`], as it contains a combination of their fields and
associated functions.

The WebSocketConn implements `Stream<Item=Result<Message, Error>>`,
and can be polled with `StreamExt::next`
*/

pub struct WebSocketConn {
    request_headers: Headers,
    path: String,
    method: Method,
    state: Extensions,
    stopper: Stopper,
    wss: Option<Wss>,
    spawn: SpawnFn,
}

impl Debug for WebSocketConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketConn")
            .field("request_headers", &self.request_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("state", &self.state)
            .field("stopper", &self.stopper)
            .field("wss", &self.wss)
            .finish()
    }
}

impl Drop for WebSocketConn {
    fn drop(&mut self) {
        if let Some(mut wss) = self.wss.take() {
            (self.spawn)(Box::pin(async move {
                trillium::log_error!(wss.close(None).await);
            }));
        }
    }
}

impl WebSocketConn {
    /// send a [`Message::Text`] variant
    pub async fn send_string(&mut self, string: impl Into<String>) {
        self.wss().send(Message::text(string)).await.ok();
    }

    /// send a [`Message::Binary`] variant
    pub async fn send_bytes(&mut self, bin: impl Into<Vec<u8>>) {
        self.wss().send(Message::binary(bin)).await.ok();
    }

    #[cfg(feature = "json")]
    /// send a [`Message::Text`] that contains json
    /// note that json messages are not actually part of the websocket specification.
    pub async fn send_json(&mut self, json: &impl serde::Serialize) -> serde_json::Result<()> {
        self.send_string(serde_json::to_string(json)?).await;
        Ok(())
    }

    pub(crate) async fn new<F>(upgrade: Upgrade, spawn: F) -> Self
    where
        F: Fn(Pin<Box<dyn Future<Output = ()> + Send + 'static>>) + Send + 'static,
    {
        let Upgrade {
            request_headers,
            path,
            method,
            state,
            buffer,
            transport,
            stopper,
        } = upgrade;

        let wss = if let Some(vec) = buffer {
            WebSocketStream::from_partially_read(transport, vec, Role::Server, None).await
        } else {
            WebSocketStream::from_raw_socket(transport, Role::Server, None).await
        };

        Self {
            request_headers,
            path,
            method,
            state,
            wss: Some(stopper.stop_stream(wss)),
            stopper,
            spawn: Box::new(spawn),
        }
    }

    /// retrieve a clone of the server's [`Stopper`]
    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    /// close the websocket connection gracefully
    pub async fn close(&mut self) {
        self.wss().close(None).await.ok();
    }

    fn wss(&mut self) -> &mut WebSocketStream<BoxedTransport> {
        self.wss.as_mut().unwrap()
    }

    /// retrieve the request headers for this conn
    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }

    /// retrieve the request path for this conn
    pub fn path(&self) -> &str {
        &self.path
    }

    /// retrieve the request method for this conn
    pub fn method(&self) -> &Method {
        &self.method
    }

    /**
    retrieve state from the state set that has been accumulated by
    trillium handlers run on the [`trillium::Conn`] before it
    became a websocket. see [`trillium::Conn::state`] for more
    information
    */
    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.state.get()
    }

    /**
    take some type T out of the state set that has been
    accumulated by trillium handlers run on the [`trillium::Conn`]
    before it became a websocket. see [`trillium::Conn::take_state`]
    for more information
    */
    pub fn take_state<T: 'static>(&mut self) -> Option<T> {
        self.state.remove()
    }
}

impl Stream for WebSocketConn {
    type Item = Result;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(wss) = &mut self.wss {
            Pin::new(wss).poll_next(cx)
        } else {
            Poll::Ready(None)
        }
    }
}
