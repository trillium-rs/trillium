//! Pure per-stream protocol state machine (RFC 9113 §5.1).
//!
//! This is the *protocol* state of a single h2 stream and nothing else: no I/O, no
//! buffers, no body-framing progress, no wakers. The driver feeds it [`StreamEvent`]s at
//! the points where a frame is sent or received and reads back whether the transition was
//! legal; all the data a stream carries (the response `Body`, recv ring, trailers, the
//! send cursor) lives a layer up, keyed by stream id on the driver side.
//!
//! Keeping this small and pure is the point. It is simultaneously the mental model, the
//! property-test oracle, and — because impossible transitions are unrepresentable — the
//! reason a whole class of consistency bugs can't arise: protocol state is never fused with
//! send-framing progress, recv buffering, or I/O bookkeeping, so no two representations of
//! the same stream can drift out of agreement.
//!
//! Design notes and the decisions behind the lenient error categories live in
//! `internal/h2-stream-state-redesign.md`.

use super::H2ErrorCode;

/// Position of a single h2 stream in the RFC 9113 §5.1 lifecycle.
///
/// The reserved (pushed) states are intentionally absent — we don't implement server push.
/// Adding it later is additive (two variants + their transitions); see the redesign doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum StreamLifecycle {
    /// Neither half opened yet. Transient: the opening HEADERS leaves it immediately. The
    /// driver only ever constructs a stream from its opening HEADERS, so in practice a
    /// stream barely exists in this state — it's kept as the natural initial value and the
    /// future per-origin construction point (push).
    #[default]
    Idle,
    /// Both halves open. A bidirectional upgrade/tunnel is just this state persisting —
    /// neither side has sent `END_STREAM` — not a distinct state.
    Open,
    /// We've framed our `END_STREAM`; the peer may still send. (§5.1 half-closed local.)
    HalfClosedLocal,
    /// The peer sent `END_STREAM`; we may still send. (§5.1 half-closed remote.)
    HalfClosedRemote,
    /// Both halves done, or the stream was reset. [`CloseReason`] records which.
    Closed { reason: CloseReason },
}

/// How a stream reached [`StreamLifecycle::Closed`]. The error *level* of a late inbound frame
/// doesn't branch on this — all post-close inbound gets a lenient stream-level
/// `STREAM_CLOSED` (see the redesign doc); the reason survives to tell the driver whether to
/// resolve the send with `Ok` vs `Err` and how to categorize the closed-streams ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloseReason {
    /// Clean close: both halves sent `END_STREAM`.
    EndStream,
    /// `RST_STREAM` in either direction, carrying the code that was sent/received.
    Reset(H2ErrorCode),
}

/// An event that transitions a stream's §5.1 state. Note what's *absent*: `WINDOW_UPDATE`,
/// `PRIORITY`, `SETTINGS`, `PING` — none cause a stream-state transition (confirmed against
/// both python-hyper/h2 and swift-nio-http2), so they never reach this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StreamEvent {
    /// A HEADERS block we framed (initial response/request, interim, or trailers).
    SendHeaders { end_stream: bool },
    /// A HEADERS block the peer sent.
    RecvHeaders { end_stream: bool },
    /// A DATA frame we framed.
    SendData { end_stream: bool },
    /// A DATA frame the peer sent.
    RecvData { end_stream: bool },
    /// We're sending `RST_STREAM`.
    SendReset(H2ErrorCode),
    /// The peer sent `RST_STREAM`.
    RecvReset(H2ErrorCode),
}

/// A transition the peer is not permitted to make. The driver turns a [`ErrorLevel::Stream`]
/// error into an `RST_STREAM` on that stream and a [`ErrorLevel::Connection`] error into a
/// connection-level GOAWAY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StreamProtocolError {
    /// Whether this is a stream-scoped or connection-scoped error.
    pub(super) level: ErrorLevel,
    /// The §7 error code to report.
    pub(super) code: H2ErrorCode,
}

impl StreamProtocolError {
    fn new(level: ErrorLevel, code: H2ErrorCode) -> Self {
        Self { level, code }
    }
}

/// Whether a [`StreamProtocolError`] should reset just the stream or tear down the
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ErrorLevel {
    /// `RST_STREAM` the offending stream; the connection continues.
    Stream,
    /// Connection error → GOAWAY.
    Connection,
}

impl StreamLifecycle {
    /// `true` once the stream is fully closed (both halves done, or reset).
    pub(super) fn is_closed(self) -> bool {
        matches!(self, Self::Closed { .. })
    }

    /// `true` once the recv half is done — the peer's `END_STREAM` has been observed, or
    /// the stream is closed. `poll_read` returns EOF here; further inbound DATA is illegal.
    pub(super) fn recv_closed(self) -> bool {
        matches!(self, Self::HalfClosedRemote | Self::Closed { .. })
    }

    /// `true` once the send half is done — our `END_STREAM` has been framed, or the stream
    /// is closed. Further outbound framing is a local bug.
    pub(super) fn send_closed(self) -> bool {
        matches!(self, Self::HalfClosedLocal | Self::Closed { .. })
    }

    /// Apply one event, transitioning the state in place. Returns `Err` for a peer protocol
    /// violation (the driver decides RST vs GOAWAY from [`ErrorLevel`]); on `Err` the state
    /// is left unchanged. Illegal *local* sends (our own desync) are absorbed with a
    /// `debug_assert` rather than surfaced — they shouldn't happen if the send pump respects
    /// this machine, and we don't tear down a connection over our own bug.
    pub(super) fn on_event(&mut self, ev: StreamEvent) -> Result<(), StreamProtocolError> {
        use CloseReason::{EndStream, Reset};
        use ErrorLevel::{Connection, Stream};
        use StreamEvent::{RecvData, RecvHeaders, RecvReset, SendData, SendHeaders, SendReset};
        use StreamLifecycle::{Closed, HalfClosedLocal, HalfClosedRemote, Idle, Open};

        // A reset from either side collapses any live stream; a reset on an already-closed
        // stream is ignored (§5.1 tolerates a late RST after close).
        if let SendReset(code) | RecvReset(code) = ev {
            if !self.is_closed() {
                *self = Closed {
                    reason: Reset(code),
                };
            }
            return Ok(());
        }

        // Some arms coincide in their *result* (e.g. `Idle`/`Open` on `SendHeaders` both yield
        // `HalfClosedLocal`/`Open`) but are kept per-state on purpose: the §5.1 structure is the
        // point, and merging would route `(Idle, SendData)` — a local desync — into the wrong arm.
        #[allow(
            clippy::match_same_arms,
            reason = "per-state arms kept for §5.1 legibility"
        )]
        let next = match (*self, ev) {
            // Opening — Idle is transient; the first HEADERS leaves it at once.
            (Idle, SendHeaders { end_stream }) => {
                if end_stream {
                    HalfClosedLocal
                } else {
                    Open
                }
            }
            (Idle, RecvHeaders { end_stream }) => {
                if end_stream {
                    HalfClosedRemote
                } else {
                    Open
                }
            }

            // Both halves open.
            (Open, SendHeaders { end_stream } | SendData { end_stream }) => {
                if end_stream {
                    HalfClosedLocal
                } else {
                    Open
                }
            }
            (Open, RecvHeaders { end_stream } | RecvData { end_stream }) => {
                if end_stream {
                    HalfClosedRemote
                } else {
                    Open
                }
            }

            // Our send half closed; only the peer may still send.
            (HalfClosedLocal, RecvHeaders { end_stream } | RecvData { end_stream }) => {
                if end_stream {
                    Closed { reason: EndStream }
                } else {
                    HalfClosedLocal
                }
            }

            // Peer's send half closed; only we may still send. (This is the bidi-upgrade
            // case once the peer half-closes: the handler keeps framing here, legally.)
            (HalfClosedRemote, SendHeaders { end_stream } | SendData { end_stream }) => {
                if end_stream {
                    Closed { reason: EndStream }
                } else {
                    HalfClosedRemote
                }
            }

            // Illegal inbound after the peer's END_STREAM, or any inbound on a closed
            // stream: lenient stream-level STREAM_CLOSED (see redesign doc decisions log —
            // permissive is the ecosystem norm; zero-length DATA after END_STREAM happens).
            (HalfClosedRemote, RecvHeaders { .. } | RecvData { .. })
            | (Closed { .. }, RecvHeaders { .. } | RecvData { .. }) => {
                return Err(StreamProtocolError::new(Stream, H2ErrorCode::StreamClosed));
            }

            // Peer DATA before any HEADERS — connection PROTOCOL_ERROR. Unreachable in
            // practice (the driver's id-level checks catch never-opened streams before the
            // lifecycle, and a stream only enters here via its opening HEADERS); encoded for
            // honesty.
            (Idle, RecvData { .. }) => {
                return Err(StreamProtocolError::new(
                    Connection,
                    H2ErrorCode::ProtocolError,
                ));
            }

            // Reject-by-default: everything left is a *local* send in a state where our send
            // half is closed or not yet open — our desync, not the peer's. Absorb + assert.
            (_, SendHeaders { .. } | SendData { .. }) => {
                debug_assert!(
                    false,
                    "illegal local h2 send transition: {self:?} <- {ev:?}"
                );
                return Ok(());
            }

            // Resets are handled before this match; no other events exist.
            (_, SendReset(_) | RecvReset(_)) => unreachable!("resets handled above"),
        };

        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CloseReason::{EndStream, Reset},
        ErrorLevel::{Connection, Stream},
        StreamEvent::{RecvData, RecvHeaders, RecvReset, SendData, SendHeaders, SendReset},
        StreamLifecycle::{self, Closed, HalfClosedLocal, HalfClosedRemote, Idle, Open},
        StreamProtocolError,
    };
    use crate::h2::H2ErrorCode;

    /// Run an event against a starting state, returning the resulting state or the error.
    fn step(
        state: StreamLifecycle,
        ev: super::StreamEvent,
    ) -> Result<StreamLifecycle, StreamProtocolError> {
        let mut state = state;
        state.on_event(ev)?;
        Ok(state)
    }

    /// Apply a sequence, asserting each step lands on the expected state.
    #[track_caller]
    fn walk(start: StreamLifecycle, steps: &[(super::StreamEvent, StreamLifecycle)]) {
        let mut state = start;
        for (ev, expected) in steps {
            state.on_event(*ev).expect("legal transition");
            assert_eq!(state, *expected, "after {ev:?}");
        }
    }

    #[test]
    fn client_request_response_lifecycle() {
        walk(
            Idle,
            &[
                (SendHeaders { end_stream: false }, Open),
                (SendData { end_stream: true }, HalfClosedLocal), // request body done
                (RecvHeaders { end_stream: false }, HalfClosedLocal), // response headers
                (RecvData { end_stream: true }, Closed { reason: EndStream }),
            ],
        );
    }

    #[test]
    fn server_request_response_lifecycle() {
        walk(
            Idle,
            &[
                (RecvHeaders { end_stream: false }, Open),
                (RecvData { end_stream: true }, HalfClosedRemote), // request body done
                (SendHeaders { end_stream: false }, HalfClosedRemote), // response headers
                (SendData { end_stream: true }, Closed { reason: EndStream }),
            ],
        );
    }

    #[test]
    fn bodyless_get_collapses_through_half_closed() {
        // Server side of a GET: request HEADERS carry END_STREAM (→ half-closed remote),
        // response HEADERS carry END_STREAM (→ closed). No DATA either way.
        walk(
            Idle,
            &[
                (RecvHeaders { end_stream: true }, HalfClosedRemote),
                (
                    SendHeaders { end_stream: true },
                    Closed { reason: EndStream },
                ),
            ],
        );
    }

    #[test]
    fn bidi_upgrade_survives_peer_half_close_then_completes() {
        // A bidirectional upgrade: response HEADERS without END_STREAM keeps the stream
        // Open, the peer half-closes its request side (→ half-closed remote), and the
        // handler keeps framing legally until it ends the stream.
        walk(
            Idle,
            &[
                (RecvHeaders { end_stream: false }, Open),
                (SendHeaders { end_stream: false }, Open), // upgrade: no END_STREAM
                (RecvData { end_stream: true }, HalfClosedRemote), // peer half-closes
                (SendData { end_stream: false }, HalfClosedRemote), // handler still writing
                (SendData { end_stream: true }, Closed { reason: EndStream }),
            ],
        );
    }

    #[test]
    fn reset_from_any_live_state_closes_with_reason() {
        for start in [Idle, Open, HalfClosedLocal, HalfClosedRemote] {
            assert_eq!(
                step(start, RecvReset(H2ErrorCode::Cancel)),
                Ok(Closed {
                    reason: Reset(H2ErrorCode::Cancel)
                }),
                "peer RST from {start:?}",
            );
            assert_eq!(
                step(start, SendReset(H2ErrorCode::InternalError)),
                Ok(Closed {
                    reason: Reset(H2ErrorCode::InternalError)
                }),
                "local RST from {start:?}",
            );
        }
    }

    #[test]
    fn late_reset_on_closed_stream_is_ignored() {
        // A RST arriving after we've already closed must not clobber the recorded reason.
        assert_eq!(
            step(Closed { reason: EndStream }, RecvReset(H2ErrorCode::Cancel)),
            Ok(Closed { reason: EndStream }),
        );
        let prior = Reset(H2ErrorCode::Cancel);
        assert_eq!(
            step(
                Closed { reason: prior },
                RecvReset(H2ErrorCode::InternalError)
            ),
            Ok(Closed { reason: prior }),
        );
    }

    #[test]
    fn inbound_after_peer_end_stream_is_lenient_stream_error() {
        // Peer already sent END_STREAM (half-closed remote); more inbound is STREAM_CLOSED
        // at stream level, and the state is left unchanged so the driver can RST + tear down
        // on its own terms.
        let err = StreamProtocolError {
            level: Stream,
            code: H2ErrorCode::StreamClosed,
        };
        assert_eq!(
            step(HalfClosedRemote, RecvData { end_stream: false }),
            Err(err)
        );
        assert_eq!(
            step(HalfClosedRemote, RecvData { end_stream: true }),
            Err(err)
        );
        assert_eq!(
            step(HalfClosedRemote, RecvHeaders { end_stream: true }),
            Err(err)
        );
    }

    #[test]
    fn inbound_on_closed_stream_is_stream_error_regardless_of_reason() {
        let err = StreamProtocolError {
            level: Stream,
            code: H2ErrorCode::StreamClosed,
        };
        assert_eq!(
            step(Closed { reason: EndStream }, RecvData { end_stream: false }),
            Err(err)
        );
        assert_eq!(
            step(
                Closed {
                    reason: Reset(H2ErrorCode::Cancel)
                },
                RecvHeaders { end_stream: true }
            ),
            Err(err),
        );
    }

    #[test]
    fn peer_data_before_headers_is_connection_error() {
        assert_eq!(
            step(Idle, RecvData { end_stream: false }),
            Err(StreamProtocolError {
                level: Connection,
                code: H2ErrorCode::ProtocolError
            }),
        );
    }

    #[test]
    #[should_panic(expected = "illegal local h2 send transition")]
    fn local_send_after_our_end_stream_asserts() {
        // We framed END_STREAM (half-closed local) and then try to send more — our desync.
        // Debug builds assert; release would absorb (state unchanged, Ok).
        let _ = step(HalfClosedLocal, SendData { end_stream: false });
    }
}
