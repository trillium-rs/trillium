use crate::{ChannelBroadcaster, ChannelCentral, ChannelEvent, ChannelHandler};
use std::ops::{Deref, DerefMut};
use trillium::{Conn, Handler, Upgrade};
use trillium_websockets::{WebSocket, WebSocketConfig};

/// Trillium handler containing a [`ChannelHandler`]
///
/// This is constructed from a [`ChannelHandler`] using [`Channel::new`]
/// and dereferences to that type.
///
/// By default, a browser can only connect from a page on the same host as the
/// socket. To serve pages from other origins, see [`Channel::allow_origins`].
#[derive(Debug)]
pub struct Channel<CH>(WebSocket<ChannelCentral<CH>>);

impl<CH> Handler for Channel<CH>
where
    CH: ChannelHandler,
{
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    async fn init(&mut self, info: &mut trillium::Info) {
        self.0.init(info).await;
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.0.before_send(conn).await
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.0.has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.0.upgrade(upgrade).await
    }
}

impl<CH: ChannelHandler> Channel<CH> {
    /// Constructs a new trillium Channel handler from the provided
    /// [`ChannelHandler`] implementation
    pub fn new(channel_handler: CH) -> Self {
        Self(WebSocket::new(ChannelCentral::new(channel_handler)))
    }

    /// Accept connections only from pages on these origins.
    ///
    /// ```
    /// # use trillium_channels::{Channel, ChannelConn, ChannelEvent, ChannelHandler};
    /// # struct Chat; impl ChannelHandler for Chat {
    /// #     async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {}
    /// # }
    /// Channel::new(Chat).allow_origins(["https://app.example.com"]);
    /// ```
    ///
    /// Origins are compared exactly, after normalizing default ports and idn hosts, never by
    /// prefix or suffix. For a family of subdomains, use
    /// [`allow_origin_fn`][Self::allow_origin_fn].
    ///
    /// A connection with no `Origin` header is allowed, as it did not come from a browser.
    ///
    /// # Panics
    ///
    /// Panics if any of the provided strings is not a url with a scheme and a host, or if it
    /// carries a path, query, fragment, or userinfo.
    pub fn allow_origins<'a>(self, origins: impl IntoIterator<Item = &'a str>) -> Self {
        Self(self.0.allow_origins(origins))
    }

    /// Accept connections for which this predicate returns true.
    ///
    /// The argument is the raw `Origin` header, so `None` (a non-browser client) stays
    /// distinguishable from `Some("null")` (a sandboxed iframe or a `file://` page, which is
    /// attacker-reachable and should generally be rejected).
    pub fn allow_origin_fn<F>(self, predicate: F) -> Self
    where
        F: Fn(Option<&str>) -> bool + Send + Sync + 'static,
    {
        Self(self.0.allow_origin_fn(predicate))
    }

    /// Accept connections from any origin, disabling the same-origin default.
    ///
    /// Any page on the web can then open a socket to this channel and act with the browser's
    /// ambient authority — the visitor's cookies are attached to the connection. Only do this if
    /// the socket is either unauthenticated or authenticated by something the page cannot
    /// replay, such as a token in the socket's query string or join payload.
    pub fn allow_any_origin(self) -> Self {
        Self(self.0.allow_any_origin())
    }

    /// Configure the websocket protocol for every connection, such as the maximum message and
    /// frame sizes.
    pub fn with_protocol_config(self, config: WebSocketConfig) -> Self {
        Self(self.0.with_protocol_config(config))
    }

    /// Retrieve a ChannelBroadcaster that can be moved elsewhere or cloned
    /// in order to trigger channel events and listen for global events.
    pub fn broadcaster(&self) -> ChannelBroadcaster {
        self.0.channel_broadcaster()
    }

    /// Send a ChannelEvent to all connected clients that subscribe to the topic
    pub fn broadcast(&self, event: impl Into<ChannelEvent>) {
        self.0.broadcast(event);
    }
}

impl<CH> Deref for Channel<CH> {
    type Target = CH;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<CH> DerefMut for Channel<CH> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
