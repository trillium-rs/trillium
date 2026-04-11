//! Sync primitive for "wait until the peer's first SETTINGS frame is applied."
//!
//! Required for callers that send extended-CONNECT requests (RFC 9220 — WebSocket-over-h3,
//! WebTransport-over-h3): the spec forbids sending a `:protocol` pseudo-header until the peer
//! has advertised `SETTINGS_ENABLE_CONNECT_PROTOCOL`. The [`PeerSettingsReady`] future parks
//! until the inbound control stream has applied the peer's SETTINGS frame, then resolves with
//! a snapshot.

use super::H3Connection;
#[cfg(feature = "unstable")]
use {
    crate::h3::H3Settings,
    event_listener::EventListener,
    std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    },
};

/// Future returned by [`H3Connection::peer_settings_ready`].
///
/// Resolves to `Some(snapshot)` once the inbound control stream has applied the peer's
/// SETTINGS frame, or to `None` if the connection was asked to shut down before SETTINGS
/// arrived. The `Option` disambiguates "peer never sent SETTINGS" from "peer sent SETTINGS
/// but did not enable the field the caller cares about", which a plain accessor on the
/// snapshot otherwise can't tell apart — both yield `None`/`false` for the underlying field.
///
/// Per RFC 9114 §7.2.4, an HTTP/3 endpoint sends SETTINGS exactly once at the start of its
/// control stream; subsequent SETTINGS frames are a connection error. The snapshot is
/// therefore stable for the life of the connection.
///
/// Multiple `PeerSettingsReady` futures can park concurrently on the same connection; all
/// wake together when the driver fires the underlying [`Event`][event_listener::Event].
#[cfg(feature = "unstable")]
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct PeerSettingsReady<'a>(
    pub(super) &'a H3Connection,
    pub(super) Option<EventListener>,
);

#[cfg(feature = "unstable")]
impl Future for PeerSettingsReady<'_> {
    type Output = Option<H3Settings>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self(connection, listener) = &mut *self;
        loop {
            if let Some(snapshot) = connection.peer_settings.get().copied() {
                return Poll::Ready(Some(snapshot));
            }
            if !connection.swansong.state().is_running() {
                return Poll::Ready(None);
            }
            let l = if let Some(l) = listener {
                l
            } else {
                let l = listener.insert(connection.peer_settings_event.listen());
                // Re-check after registering — same load/register/recheck idiom — so a notify
                // racing the registration isn't lost.
                if let Some(snapshot) = connection.peer_settings.get().copied() {
                    return Poll::Ready(Some(snapshot));
                }
                if !connection.swansong.state().is_running() {
                    return Poll::Ready(None);
                }
                l
            };
            std::task::ready!(Pin::new(l).poll(cx));
            *listener = None;
        }
    }
}

impl H3Connection {
    /// Park until the inbound control stream has applied the peer's SETTINGS frame.
    ///
    /// The returned [`PeerSettingsReady`] future resolves to `Some(snapshot)` once a peer
    /// SETTINGS frame has been applied, or to `None` if the connection was asked to shut
    /// down before SETTINGS arrived. On a pooled connection that has already exchanged
    /// SETTINGS, the future resolves on the first poll; only fresh, just-handshaked
    /// connections actually park.
    ///
    /// Required for callers that send extended-CONNECT requests (RFC 9220 — WebSocket-over-h3,
    /// WebTransport-over-h3): the spec forbids sending a `:protocol` pseudo-header until the
    /// peer has advertised `SETTINGS_ENABLE_CONNECT_PROTOCOL`. Awaiting this future and then
    /// inspecting the returned [`H3Settings`] snapshot resolves the "peer hasn't sent SETTINGS
    /// yet" vs "peer sent SETTINGS without the field" ambiguity in a single step:
    ///
    /// ```ignore
    /// let Some(settings) = h3.peer_settings_ready().await else {
    ///     // connection shut down before SETTINGS arrived
    /// };
    /// if settings.enable_connect_protocol() != Some(true) {
    ///     // peer doesn't support extended CONNECT
    /// }
    /// ```
    ///
    /// For a non-blocking accessor that returns the current snapshot or `None` if SETTINGS
    /// have not yet been applied, see [`Self::peer_settings`].
    ///
    /// Multiple awaiters on the same connection are supported — internally backed by an
    /// [`Event`][event_listener::Event] rather than a single waker.
    #[cfg(feature = "unstable")]
    pub fn peer_settings_ready(&self) -> PeerSettingsReady<'_> {
        PeerSettingsReady(self, None)
    }

    /// Driver-side: wake every parked [`PeerSettingsReady`] future. Called when the peer's
    /// SETTINGS frame has been applied (waiters resolve to `Some(snapshot)`) and when the
    /// connection is shutting down (waiters resolve to `None`). Idempotent.
    pub(super) fn wake_peer_settings_waiters(&self) {
        self.peer_settings_event.notify(usize::MAX);
    }
}
