//! # An implementation of phoenix channels for trillium.rs
//!
//! Channels are a means of distributing events in soft-realtime to
//! connected websocket clients, including topic-subscription-based
//! fanout.
//!
//! From the [phoenix docs](https://hexdocs.pm/phoenix/channels.html),
//!
//! Some possible use cases include:
//!
//! Chat rooms and APIs for messaging apps
//! Breaking news, like "a goal was scored" or "an earthquake is coming"
//! Tracking trains, trucks, or race participants on a map
//! Events in multiplayer games
//! Monitoring sensors and controlling lights
//! Notifying a browser that a page's CSS or JavaScript has changed
//! (this is handy in development)
//! Conceptually, Channels are pretty simple.
//!
//! First, clients connect to the server using WebSockets. Once connected,
//! they join one or more topics. For example, to interact with a public
//! chat room clients may join a topic called public_chat, and to receive
//! updates from a product with ID 7, they may need to join a topic called
//! product_updates:7.
//!
//! Clients can push messages to the topics they've joined, and can also
//! receive messages from them. The other way around, Channel servers
//! receive messages from their connected clients, and can push messages
//! to them too.
//!
//!
//! ## Current known differences from phoenix channels:
//!
//! ### No long-polling support
//!
//! Phoenix channels support long polling transports as well as
//! websockets. As most modern browsers and http clients support
//! websockets, as of the current version, trillium channels exclusively
//! are built on them. The design should be flexible to support long
//! polling if it is eventually needed.
//!
//! ### No multi-server synchronization yet
//!
//! Phoenix channels support running several server nodes and distributing
//! all broadcast messages between them. This will be straightforward to
//! add to trillium channels in a future revision, but the current
//! implementation does not synchronize messages across servers. However,
//! in the mean time, you can use [`Channel::broadcaster`] to return a
//! [`ChannelBroadcaster`] that can be used to publish and subscribe to
//! messages between servers using whatever distribution mechanism is
//! appropriate for your application and deployment. Open a discussion on
//! the trillium repo for ideas on how this might work for you.
//!
//!
//! ### Event routing is handled in user code
//!
//! Phoenix channels has a notion of registering channel handlers for
//! different topics, so an implementation might involve registering a
//! RoomChannel for `rooms:*`. Trillium channels does not currently
//! provide this routing/matching behavior, but will likely do so
//! eventually.
//!
//!
//! ## Simple Example: Chat App
//!
//! ```
//! use trillium_channels::{channel, ChannelConn, ChannelEvent, ChannelHandler};
//!
//! struct ChatChannel;
//! impl ChannelHandler for ChatChannel {
//!     async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
//!         match event.topic() {
//!             "rooms:lobby" => {
//!                 conn.allow_join(&event, &()).await;
//!                 conn.broadcast(("rooms:lobby", "user:entered"));
//!             }
//!
//!             _ => {}
//!         }
//!     }
//!
//!     async fn incoming_message(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
//!         match (event.topic(), event.event()) {
//!             ("rooms:lobby", "new:msg") => conn.broadcast(event),
//!             _ => {}
//!         }
//!     }
//! }
//!
//! // fn main() {
//! //    trillium_smol::run(channel(ChatChannel));
//! // }
//! ```
//!
//! See channels/examples/channels.rs for a fully functional example that uses the front-end from [the phoenix chat example](https://github.com/chrismccord/phoenix_chat_example).

#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_debug_implementations,
    nonstandard_style,
    rustdoc::missing_crate_level_docs,
    unused_qualifications
)]
#![warn(missing_docs)]

mod channel_central;
pub(crate) use channel_central::ChannelCentral;

mod channel_broadcaster;
pub use channel_broadcaster::ChannelBroadcaster;

mod channel_event;
pub use channel_event::ChannelEvent;

mod channel_client;
pub use channel_client::ChannelClient;

pub(crate) mod client_receiver;

mod channel_handler;
pub use channel_handler::ChannelHandler;

mod channel;
pub use channel::Channel;

pub(crate) mod subscriptions;

mod channel_conn;
pub use channel_conn::ChannelConn;

mod version;
pub use version::Version;

/// This macro provides a convenient constructor for a
/// [`ChannelEvent`]. It is called with a topic, an event, and an optional
/// inline json payload.
///
///
/// ```
/// let event = trillium_channels::event!("some:topic", "some:event");
/// assert_eq!(event.topic(), "some:topic");
/// assert_eq!(event.event(), "some:event");
/// assert_eq!(serde_json::to_string(event.payload()).unwrap(), "{}");
///
///
/// let event = trillium_channels::event!("some:topic", "some:event", { "payload": ["any", "json"] });
/// assert_eq!(event.topic(), "some:topic");
/// assert_eq!(event.event(), "some:event");
/// assert_eq!(serde_json::to_string(event.payload()).unwrap(), r#"{"payload":["any","json"]}"#);
/// ```
#[macro_export]
macro_rules! event {
    ($topic:expr, $event:expr) => {
        $crate::ChannelEvent::new($topic, $event, &())
    };

    ($topic:expr, $event:expr, $($json:tt)+) => {
        $crate::ChannelEvent::new($topic, $event, &$crate::json!($($json)+))
    };
}

pub use serde_json::json;

/// Constructs a new [`Channel`] trillium handler from the provided
/// [`ChannelHandler`]. This is an alias for [`Channel::new`]
pub fn channel<CH: ChannelHandler>(channel_handler: CH) -> Channel<CH> {
    Channel::new(channel_handler)
}
