//! Sync primitive for "wait until the peer's first SETTINGS frame is applied."
//!
//! Required for callers that send extended-CONNECT requests (RFC 8441 §3 — WebSocket-over-h2):
//! the spec forbids sending a `:protocol` pseudo-header until the peer has advertised
//! `SETTINGS_ENABLE_CONNECT_PROTOCOL`. The [`PeerSettings`] future parks until the driver has
//! applied at least one peer SETTINGS frame, then resolves with a snapshot.

use super::H2Connection;
use std::sync::atomic::Ordering;
#[cfg(feature = "unstable")]
use {
    crate::h2::H2Settings,
    event_listener::EventListener,
    std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    },
};

/// Future returned by [`H2Connection::peer_settings`].
///
/// Resolves to `Some(snapshot)` once the driver has applied the peer's first SETTINGS frame,
/// or to `None` if the connection was asked to shut down before any SETTINGS arrived. The
/// `Option` disambiguates "peer never sent SETTINGS" from "peer sent SETTINGS but didn't
/// enable the field the caller cares about" — both yield `None` from a plain field accessor.
///
/// The snapshot is taken at resolve time; the peer may send further SETTINGS frames later.
/// For limits that can change over time (like `MAX_CONCURRENT_STREAMS`), follow up with
/// [`H2Connection::peer_settings_snapshot`].
///
/// Multiple `PeerSettings` futures can park concurrently on the same connection; all wake
/// together.
#[cfg(feature = "unstable")]
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct PeerSettings<'a>(
    pub(super) &'a H2Connection,
    pub(super) Option<EventListener>,
);

#[cfg(feature = "unstable")]
impl Future for PeerSettings<'_> {
    type Output = Option<H2Settings>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self(connection, listener) = &mut *self;
        loop {
            if let Some(snapshot) = connection.peer_settings_snapshot() {
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
                if let Some(snapshot) = connection.peer_settings_snapshot() {
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

impl H2Connection {
    /// Park until the driver has applied the peer's first SETTINGS frame. On a pooled
    /// connection that has already exchanged SETTINGS, the future resolves on the first
    /// poll; only fresh, just-handshaked connections actually park.
    ///
    /// Resolves to `Some(snapshot)` once a peer SETTINGS frame has been applied, or to
    /// `None` if the connection was asked to shut down before SETTINGS arrived. The
    /// snapshot is taken at resolve time; subsequent peer SETTINGS frames are not
    /// reflected — for limits that can change (`MAX_CONCURRENT_STREAMS`), follow up
    /// with [`Self::peer_settings_snapshot`]. Multiple awaiters on the same connection
    /// are supported.
    ///
    /// Canonical use for extended CONNECT (WebSocket-over-h2):
    ///
    /// ```ignore
    /// let Some(settings) = h2.peer_settings().await else {
    ///     // connection shut down before SETTINGS arrived
    /// };
    /// if settings.enable_connect_protocol() != Some(true) {
    ///     // peer doesn't support extended CONNECT
    /// }
    /// ```
    #[cfg(feature = "unstable")]
    pub fn peer_settings(&self) -> PeerSettings<'_> {
        PeerSettings(self, None)
    }

    /// A snapshot of the peer's most recently applied SETTINGS, or `None` if the peer hasn't
    /// sent any SETTINGS frame yet on this connection. The returned [`H2Settings`] is a
    /// `Copy` value owned by the caller; subsequent peer SETTINGS frames will not be
    /// reflected. For a synchronization primitive that parks until the first frame arrives,
    /// see [`Self::peer_settings`].
    #[cfg(feature = "unstable")]
    pub fn peer_settings_snapshot(&self) -> Option<H2Settings> {
        // Acquire-loaded: pairs with the Release-store in `apply_peer_settings` so the
        // settings written under the peer_settings mutex are visible without taking it.
        self.peer_settings_received
            .load(Ordering::Acquire)
            .then(|| *self.current_peer_settings())
    }

    /// Driver-side: a peer SETTINGS frame has just been applied. Latches the
    /// `peer_settings_received` flag and wakes every parked [`PeerSettings`] future.
    /// Idempotent — calling more than once on the same connection is harmless; spurious
    /// wakes are absorbed by the future's poll loop.
    pub(in crate::h2) fn note_peer_settings(&self) {
        self.peer_settings_received.store(true, Ordering::Release);
        self.peer_settings_event.notify(usize::MAX);
    }

    /// Driver-side: the connection is closing. Wakes every parked [`PeerSettings`] future so
    /// callers awaiting the peer's first SETTINGS observe the shutdown rather than
    /// blocking forever.
    pub(in crate::h2) fn wake_peer_settings_waiters(&self) {
        self.peer_settings_event.notify(usize::MAX);
    }
}
