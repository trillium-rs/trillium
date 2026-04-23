/// Absolute upper bound on transient frame buffers — a backstop against a peer that advertises
/// an absurd frame size. Independent of `HttpConfig::h2_max_frame_size` (which we advertise and
/// enforce against incoming frames); this is just the ceiling on our own decode buffer to
/// prevent runaway allocation under an adversarial peer.
pub(super) const MAX_BUFFER_SIZE: usize = 1 << 20;

/// RFC 9113 §6.9.2 baseline connection-level flow-control window — 65535 octets for both
/// directions, unchanged by SETTINGS. Used as the starting value for our send-side window
/// (credited via peer `WINDOW_UPDATE(0)`) and for our recv-side window before we emit the
/// initial raising `WINDOW_UPDATE(0)` to `h2_initial_connection_window_size`.
pub(super) const INITIAL_CONNECTION_RECV_WINDOW: i64 = 65_535;

/// Hard ceiling on the DATA payload we'll emit in a single frame even if the peer
/// advertises a larger `MAX_FRAME_SIZE`. Bounds `body_scratch` so a permissive peer can't
/// steer us into oversized allocations; the protocol only requires we not *exceed* the
/// peer's advertised max, which starts at the RFC 9113 §6.5.2 default of 16 KiB.
pub(super) const MAX_DATA_CHUNK_SIZE: u32 = 16_384;

/// RFC 9113 §6.9.1: a flow-control window MUST NOT exceed `2^31 - 1`. If a
/// `WINDOW_UPDATE` would push it past that maximum, the peer has misbehaved — we emit
/// `FLOW_CONTROL_ERROR` at the appropriate level (connection or stream).
pub(super) const MAX_FLOW_CONTROL_WINDOW: i64 = (1 << 31) - 1;
