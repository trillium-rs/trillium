//! Per-stream lifecycle state machine.
//!
//! Replaces the previous bag of orthogonal `AtomicBool` + `Mutex<Option<T>>` slots on
//! [`StreamState`][super::transport::StreamState] with a single [`Mutex<StreamLifecycle>`]
//! whose variants are the legal cross-task-visible states a stream can be in.
//!
//! # Why this exists
//!
//! The previous representation let any combination of flags coexist on the wire, and
//! code that needed to make state decisions had to inspect several fields in a specific
//! order to avoid races. The ordering was implicit, easy to get wrong, and the source of
//! several recurring bug classes (Drop-on-upgrade with zero-body request, trailers
//! stranding mid-body, Closing → Drained predicate gaps). Encoding the legal states as
//! enum variants makes the impossible combinations structurally unreachable and makes
//! every decision site a flat exhaustive match.
//!
//! # What stays outside this enum
//!
//! Data and wake plumbing that's stable across multiple lifecycle states stays as
//! sibling fields on [`StreamState`][super::transport::StreamState]:
//!
//! - The recv-side ring buffer, recv waker, `bytes_consumed` flow-control counter, and `trailers`
//!   slot — all recv-side data, populated by the driver and drained by the handler. Independent of
//!   the send-side state machine.
//! - The upgrade outbound buffer (`outbound`) + its wakers — `H2OutboundReader` holds an
//!   `Arc<StreamState>` and needs a stable reference, not one buried inside an enum variant that
//!   can transition out from under it.
//! - The conn-task → driver mailbox (`needs_servicing: AtomicBool`) and the driver → conn-task
//!   completion signal (`send.completed` + `send.completion_waker`). Wake plumbing, not state.
//!
//! # Reachable states (server role)
//!
//! Most cross-products of `SendState × recv_eof` are reachable. The only one that isn't
//! a steady state is "send framing complete with recv still open" — the server tears the
//! stream down as soon as `END_STREAM` goes out, so reaching that combination triggers
//! an immediate transition out of `Active` (server removes from the streams map; client
//! transitions to [`StreamLifecycle::AwaitingRelease`] to await the application drop).

use super::H2ErrorCode;
use crate::{Body, Headers, headers::hpack::PseudoHeaders};

/// Position of a single h2 stream in its lifecycle. The sole cross-task-visible state
/// slot on [`StreamState`][super::transport::StreamState]; held under a `Mutex` so
/// observe-then-act sequences are atomic.
#[derive(Debug)]
pub(super) enum StreamLifecycle {
    /// The send side hasn't been submitted yet (the handler is still running). `recv_eof`
    /// tracks whether the peer has finished its body.
    Idle { recv_eof: bool },

    /// The conn task has staged a response submission. The driver picks it up in its
    /// next `service_handler_signals` tick and transitions to either [`Self::Sending`]
    /// (normal response) or [`Self::UpgradeOpen`] (extended-CONNECT upgrade) based on
    /// the submission's `is_upgrade` flag.
    Submitted {
        submission: Box<Submission>,
        recv_eof: bool,
    },

    /// Normal response framing in progress. The driver-private `SendCursor` carries the
    /// fine-grained phase (Headers / Body / Trailers); only the coarse "send pump is
    /// active on this stream" status is visible here.
    Sending { recv_eof: bool },

    /// Extended-CONNECT upgrade body framing in progress. Response HEADERS already on
    /// the wire (with `END_STREAM = false`); the cursor pumps bytes from the upgrade
    /// handler's outbound buffer as DATA frames. `H2Transport::Drop` in this state
    /// schedules graceful close — the variant *is* the answer, no separate
    /// `graceful_drop` flag needed.
    UpgradeOpen { recv_eof: bool },

    /// Upgrade handler has requested close (either by dropping the transport, calling
    /// `poll_close` on it, or calling `send_trailers`). Outbound buffer drains, then the
    /// send pump emits trailing HEADERS (if `pending_trailers` is `Some`) or
    /// `DATA(END_STREAM)` as the stream terminator.
    UpgradeClosing {
        recv_eof: bool,
        pending_trailers: Option<Headers>,
    },

    /// Conn-task code (or the driver itself, for a malformed peer trailer block) has
    /// requested `RST_STREAM(code)`. The driver picks this up in its next
    /// `service_handler_signals` tick, emits the frame, and transitions to
    /// [`Self::Reset`]. First-RST-wins: subsequent reset requests on the same stream are
    /// no-ops, mirroring the original `pending_reset: Mutex<Option<H2ErrorCode>>` slot's
    /// `is_none()` guard.
    ResetRequested(H2ErrorCode),

    /// `RST_STREAM(code)` is on the wire (sent or received). Terminal: the stream's
    /// driver-side `SendCursor` (if any) is discarded; the stream is removed from the
    /// connection's stream map. The carried `H2ErrorCode` is informational (shows up in
    /// the variant's `Debug` output for trace logs); the wire frame's code is what the
    /// peer sees.
    Reset(#[allow(dead_code)] H2ErrorCode),

    /// Send half wire-closed: the `END_STREAM` terminator (empty `DATA(END_STREAM)` or a
    /// trailing HEADERS block) has been framed, but the stream is still in the map. This
    /// is HTTP/2 half-closed (local) (RFC 9113 §5.1): the server holds here awaiting the
    /// peer's `END_STREAM`, and the client holds here (with `recv_eof: true`) for
    /// post-EOF trailer/response access until the application drops its transport.
    ///
    /// This is the *only* "send is done on the wire" signal — distinct from
    /// [`SendState::submit_resolved`][super::transport::SendState::submit_resolved], which
    /// resolves the conn task's [`SubmitSend`][super::SubmitSend] future and fires *early*
    /// on the upgrade path (at handoff, while the stream stays `UpgradeOpen`). Teardown
    /// decisions read this variant, never that flag — conflating the two tore down open
    /// bidi upgrade streams the moment the peer half-closed its request side.
    LocalClosed { recv_eof: bool },

    /// Client-role terminal: both halves wire-closed (send `END_STREAM` emitted, peer
    /// `END_STREAM` observed), but the application still holds the per-stream
    /// `H2Transport` (e.g. to read trailers post-EOF). The driver picks this up in its
    /// next `service_handler_signals` tick and removes the stream from both maps. Server
    /// streams are removed eagerly when send completes and never reach this state.
    AwaitingRelease,
}

impl Default for StreamLifecycle {
    fn default() -> Self {
        Self::Idle { recv_eof: false }
    }
}

impl StreamLifecycle {
    /// `true` while this stream has work pending — anything but the terminal `Reset`
    /// and `AwaitingRelease` variants counts as in-flight. Used by the driver's
    /// `Closing → Drained` gate (with [`Self::has_pending_recv`]) to keep the recv and
    /// send pumps running while any stream is still active.
    #[allow(
        dead_code,
        reason = "kept as a documenting predicate; specific call sites currently use \
                  `has_active_send` + `has_pending_recv` directly"
    )]
    pub(super) fn is_in_flight(&self) -> bool {
        !matches!(
            self,
            Self::Reset(_) | Self::AwaitingRelease | Self::LocalClosed { recv_eof: true }
        )
    }

    /// `true` once both halves are wire-closed: the send `END_STREAM` terminator has been
    /// framed (`LocalClosed`) *and* the peer's `END_STREAM` has been observed. The single
    /// teardown predicate the both-done close paths consult — replacing the former
    /// `send.completed && recv_eof` read that misfired on open upgrade streams.
    pub(super) fn is_fully_closed(&self) -> bool {
        matches!(self, Self::LocalClosed { recv_eof: true })
    }

    /// `true` if the stream is wire-closed from the connection's accounting perspective —
    /// fully closed, reset, or awaiting application release. Such streams no longer count
    /// against the peer's `MAX_CONCURRENT_STREAMS` even while the application still holds
    /// the transport.
    pub(super) fn is_wire_closed(&self) -> bool {
        matches!(
            self,
            Self::LocalClosed { recv_eof: true } | Self::AwaitingRelease | Self::Reset(_)
        )
    }

    /// Mark the send half wire-closed — the `END_STREAM` terminator has been framed.
    /// Carries the currently-observed `recv_eof` into [`Self::LocalClosed`]. No-op on the
    /// terminal teardown variants so a late call can't resurrect a reset/released stream.
    pub(super) fn mark_send_closed(&mut self) {
        if matches!(
            self,
            Self::Reset(_) | Self::ResetRequested(_) | Self::AwaitingRelease
        ) {
            return;
        }
        let recv_eof = self.recv_eof();
        *self = Self::LocalClosed { recv_eof };
    }

    /// `true` if the peer still owes us body bytes — the recv half is in `Open` state
    /// (no `END_STREAM` observed) on one of the active variants. The terminal variants
    /// (`Reset`, `AwaitingRelease`) and `ResetRequested` all answer `false` because
    /// recv is no longer interesting once the stream is being torn down.
    pub(super) fn has_pending_recv(&self) -> bool {
        matches!(
            self,
            Self::Idle { recv_eof: false }
                | Self::Submitted {
                    recv_eof: false,
                    ..
                }
                | Self::Sending { recv_eof: false }
                | Self::UpgradeOpen { recv_eof: false }
                | Self::UpgradeClosing {
                    recv_eof: false,
                    ..
                }
                | Self::LocalClosed { recv_eof: false }
        )
    }

    /// `true` if the recv side has observed `END_STREAM`. Convenience accessor for
    /// `H2Transport::poll_read`'s EOF check, which previously consulted
    /// `recv.eof: AtomicBool` directly.
    pub(super) fn recv_eof(&self) -> bool {
        match self {
            Self::Idle { recv_eof }
            | Self::Submitted { recv_eof, .. }
            | Self::Sending { recv_eof }
            | Self::UpgradeOpen { recv_eof }
            | Self::UpgradeClosing { recv_eof, .. }
            | Self::LocalClosed { recv_eof } => *recv_eof,
            // Terminal states: stream is torn down or about to be; recv is no longer
            // meaningful, but report eof so any lingering `poll_read` returns Ready(0)
            // rather than parking forever.
            Self::ResetRequested(_) | Self::Reset(_) | Self::AwaitingRelease => true,
        }
    }

    /// `true` if a `SendCursor` is (or could be) active in the driver's private map for
    /// this stream — the send-side equivalent of [`Self::has_pending_recv`]. Counts the
    /// post-submission-but-pre-pickup `Submitted` variant as well, since the driver
    /// will build a cursor for it on its next tick.
    pub(super) fn has_active_send(&self) -> bool {
        matches!(
            self,
            Self::Submitted { .. }
                | Self::Sending { .. }
                | Self::UpgradeOpen { .. }
                | Self::UpgradeClosing { .. }
        )
    }

    /// Mark the recv half as having observed peer `END_STREAM` (a DATA frame with
    /// `END_STREAM = 1`, or a trailing HEADERS block). Idempotent and a no-op on terminal
    /// variants — once the stream is torn down or already released, recv state is no
    /// longer meaningful.
    pub(super) fn mark_recv_eof(&mut self) {
        match self {
            Self::Idle { recv_eof }
            | Self::Submitted { recv_eof, .. }
            | Self::Sending { recv_eof }
            | Self::UpgradeOpen { recv_eof }
            | Self::UpgradeClosing { recv_eof, .. }
            | Self::LocalClosed { recv_eof } => *recv_eof = true,
            Self::ResetRequested(_) | Self::Reset(_) | Self::AwaitingRelease => {}
        }
    }
}

/// What the conn task hands the driver to begin a send on a stream — carried as the
/// payload of [`StreamLifecycle::Submitted`] until the driver picks it up.
///
/// `body` carries either a normal response body or, for extended-CONNECT (RFC 8441)
/// upgrades, a streaming body that reads from the upgrade outbound buffer. Trailers (if
/// any) come from [`Body::trailers`] after drain — not a separate field.
///
/// `is_upgrade` selects the driver's completion semantics on pickup: normal submissions
/// transition `Submitted → Sending` and signal completion after the body is fully on the
/// wire; upgrade submissions transition `Submitted → UpgradeOpen` and signal completion
/// as soon as the response HEADERS frame is flushed, letting `Conn::send_h2` return so
/// the runtime can dispatch `Handler::upgrade`.
#[derive(Debug)]
pub(super) struct Submission {
    /// Owned pseudo-headers. Combined with `headers` at pickup to form a `FieldSection`
    /// which is HPACK-encoded synchronously by the driver against the live dynamic-table
    /// state.
    pub(super) pseudos: PseudoHeaders<'static>,
    /// Owned headers for the block.
    pub(super) headers: Headers,
    /// Response/request body. `None` causes the HEADERS frame to carry `END_STREAM` and
    /// no DATA to be emitted.
    pub(super) body: Option<Body>,
    /// Selects the upgrade-completion semantics described in the type doc above.
    pub(super) is_upgrade: bool,
}

impl Submission {
    /// Borrow this submission's headers as a [`FieldSection`][crate::headers::hpack::FieldSection]
    /// for encoding.
    pub(super) fn field_section(&self) -> crate::headers::hpack::FieldSection<'_> {
        crate::headers::hpack::FieldSection::new(self.pseudos.clone(), &self.headers)
    }
}
