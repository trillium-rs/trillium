use super::H2ErrorCode;
use crate::HttpConfig;
use fieldwork::Fieldwork;

/// `SETTINGS_HEADER_TABLE_SIZE` (RFC 9113 §6.5.2).
const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
/// `SETTINGS_ENABLE_PUSH` (RFC 9113 §6.5.2).
const SETTINGS_ENABLE_PUSH: u16 = 0x2;
/// `SETTINGS_MAX_CONCURRENT_STREAMS` (RFC 9113 §6.5.2).
const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
/// `SETTINGS_INITIAL_WINDOW_SIZE` (RFC 9113 §6.5.2).
const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
/// `SETTINGS_MAX_FRAME_SIZE` (RFC 9113 §6.5.2).
const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
/// `SETTINGS_MAX_HEADER_LIST_SIZE` (RFC 9113 §6.5.2).
const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

/// The upper bound on `SETTINGS_INITIAL_WINDOW_SIZE` (RFC 9113 §6.5.2).
const MAX_INITIAL_WINDOW_SIZE: u32 = (1 << 31) - 1;
/// The lower bound on `SETTINGS_MAX_FRAME_SIZE` (RFC 9113 §6.5.2).
const MIN_MAX_FRAME_SIZE: u32 = 16_384;
/// The upper bound on `SETTINGS_MAX_FRAME_SIZE` (RFC 9113 §6.5.2).
const MAX_MAX_FRAME_SIZE: u32 = (1 << 24) - 1;

/// Each setting on the wire is (id:u16, value:u32).
const SETTING_ENTRY_LEN: usize = 6;

/// H2 connection settings per RFC 9113 §6.5.2.
///
/// `None` fields mean the setting was absent, implying the RFC default
/// (4096 for header table, 1 for push, unlimited for concurrent streams / header list size,
/// 65535 for initial window, 16384 for max frame size).
///
/// Use [`H2Settings::server_defaults`] for a reasonable outgoing configuration; use [`decode`] for
/// incoming settings.
///
/// [`decode`]: H2Settings::decode
#[derive(Clone, Copy, Eq, Fieldwork, Default)]
#[fieldwork(get, set, with(option_set_some))]
pub(crate) struct H2Settings {
    /// The maximum size of the HPACK dynamic header table the peer may use when encoding.
    ///
    /// Default: 4096.
    header_table_size: Option<u32>,

    /// Whether the peer is permitted to initiate server push.
    ///
    /// Default: true. Trillium always advertises `Some(false)` from [`Self::from_config`]
    /// because we never send `PUSH_PROMISE`; a peer that advertises `Some(true)` is a protocol
    /// error for a server since clients cannot push.
    enable_push: Option<bool>,

    /// The maximum number of concurrent streams the peer may initiate.
    ///
    /// Default: unlimited. RFC 9113 recommends at least 100.
    max_concurrent_streams: Option<u32>,

    /// The initial flow-control window size for streams, in octets.
    ///
    /// Default: 65535. Must not exceed 2^31 - 1.
    initial_window_size: Option<u32>,

    /// The maximum frame payload size the peer is willing to receive, in octets.
    ///
    /// Default: 16384. Must be in [16384, 2^24 - 1].
    max_frame_size: Option<u32>,

    /// The maximum size of a header list the peer is willing to receive, in octets.
    ///
    /// Default: unlimited.
    max_header_list_size: Option<u32>,

    // GREASE setting included in encoded output per RFC 8701 when non-zero.
    // Chosen at construction time so encoded_len() and encode() agree.
    // Zero when decoded from a peer (GREASE is skipped during decode).
    #[field = false]
    grease_id: u16,

    #[field = false]
    grease_value: u32,
}

impl std::fmt::Debug for H2Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H2Settings")
            .field("header_table_size", &self.header_table_size)
            .field("enable_push", &self.enable_push)
            .field("max_concurrent_streams", &self.max_concurrent_streams)
            .field("initial_window_size", &self.initial_window_size)
            .field("max_frame_size", &self.max_frame_size)
            .field("max_header_list_size", &self.max_header_list_size)
            .finish_non_exhaustive()
    }
}

/// `PartialEq` ignores GREASE — it's an encoding detail, not semantically meaningful.
impl PartialEq for H2Settings {
    fn eq(&self, other: &Self) -> bool {
        self.header_table_size == other.header_table_size
            && self.enable_push == other.enable_push
            && self.max_concurrent_streams == other.max_concurrent_streams
            && self.initial_window_size == other.initial_window_size
            && self.max_frame_size == other.max_frame_size
            && self.max_header_list_size == other.max_header_list_size
    }
}

impl H2Settings {
    /// [`max_frame_size`][Self::max_frame_size] with the RFC 9113 §6.5.2 default of 16384
    /// applied when the field is `None`. Use this when you need a concrete value for
    /// framing.
    pub(crate) fn effective_max_frame_size(&self) -> u32 {
        self.max_frame_size.unwrap_or(16_384)
    }

    /// [`initial_window_size`][Self::initial_window_size] with the RFC 9113 §6.5.2 default
    /// of 65535 applied when the field is `None`. Use this when seeding a new stream's send
    /// window.
    pub(crate) fn effective_initial_window_size(&self) -> u32 {
        self.initial_window_size.unwrap_or(65_535)
    }

    /// Build an outgoing SETTINGS frame from the h2 fields of an [`HttpConfig`][crate::HttpConfig].
    ///
    /// Disables server push (`SETTINGS_ENABLE_PUSH` = 0, trillium never sends `PUSH_PROMISE`) and
    /// selects a random GREASE setting per RFC 8701 for forward-compat checking. The remaining
    /// advertised values come from the config:
    ///
    /// | Setting | Source |
    /// |---|---|
    /// | `SETTINGS_INITIAL_WINDOW_SIZE` | `h2_initial_stream_window_size` |
    /// | `SETTINGS_MAX_CONCURRENT_STREAMS` | `h2_max_concurrent_streams` |
    /// | `SETTINGS_MAX_FRAME_SIZE` | `h2_max_frame_size` |
    /// | `SETTINGS_MAX_HEADER_LIST_SIZE` | `h2_max_header_list_size` |
    pub(crate) fn from_config(config: &HttpConfig) -> Self {
        // RFC 8701: the reserved GREASE settings are 0x0A0A, 0x1A1A, ..., 0xFAFA.
        let n = u16::from(fastrand::u8(0..16));
        let grease_id = 0x0A0A | (n << 12) | (n << 4);
        Self {
            enable_push: Some(false),
            max_concurrent_streams: Some(config.h2_max_concurrent_streams()),
            max_header_list_size: Some(config.h2_max_header_list_size()),
            initial_window_size: Some(config.h2_initial_stream_window_size()),
            max_frame_size: Some(config.h2_max_frame_size()),
            grease_id,
            grease_value: fastrand::u32(..),
            ..Default::default()
        }
    }

    /// A reasonable outgoing settings frame for a trillium server, using the default
    /// `HttpConfig`. Convenience wrapper around [`Self::from_config`] for tests and ad-hoc
    /// use; production code builds via `from_config` with the real configured values.
    #[cfg(test)]
    pub(crate) fn server_defaults() -> Self {
        Self::from_config(&HttpConfig::DEFAULT)
    }

    /// The number of bytes this settings payload will occupy when encoded.
    pub(crate) fn encoded_len(&self) -> usize {
        let mut entries = 0;
        if self.header_table_size.is_some() {
            entries += 1;
        }
        if self.enable_push.is_some() {
            entries += 1;
        }
        if self.max_concurrent_streams.is_some() {
            entries += 1;
        }
        if self.initial_window_size.is_some() {
            entries += 1;
        }
        if self.max_frame_size.is_some() {
            entries += 1;
        }
        if self.max_header_list_size.is_some() {
            entries += 1;
        }
        if self.grease_id != 0 {
            entries += 1;
        }
        entries * SETTING_ENTRY_LEN
    }

    /// Decode settings from a SETTINGS frame payload.
    ///
    /// Payload length is expected to already be a multiple of 6 octets (the outer frame codec
    /// checks this and raises `FRAME_SIZE_ERROR` otherwise).
    ///
    /// # Errors
    ///
    /// Returns an [`H2ErrorCode`] if any setting has an out-of-range value:
    /// - `PROTOCOL_ERROR` for an `ENABLE_PUSH` value other than 0 or 1, or a `MAX_FRAME_SIZE`
    ///   outside \[16384, 2^24 - 1\].
    /// - `FLOW_CONTROL_ERROR` for an `INITIAL_WINDOW_SIZE` above 2^31 - 1.
    /// - `FRAME_SIZE_ERROR` if the payload length is not a multiple of 6.
    pub(crate) fn decode(payload: &[u8]) -> Result<Self, H2ErrorCode> {
        if !payload.len().is_multiple_of(SETTING_ENTRY_LEN) {
            return Err(H2ErrorCode::FrameSizeError);
        }
        let mut settings = Self::default();
        for entry in payload.chunks_exact(SETTING_ENTRY_LEN) {
            let id = u16::from_be_bytes([entry[0], entry[1]]);
            let value = u32::from_be_bytes([entry[2], entry[3], entry[4], entry[5]]);
            match id {
                SETTINGS_HEADER_TABLE_SIZE => settings.header_table_size = Some(value),
                SETTINGS_ENABLE_PUSH => match value {
                    0 => settings.enable_push = Some(false),
                    1 => settings.enable_push = Some(true),
                    _ => return Err(H2ErrorCode::ProtocolError),
                },
                SETTINGS_MAX_CONCURRENT_STREAMS => settings.max_concurrent_streams = Some(value),
                SETTINGS_INITIAL_WINDOW_SIZE => {
                    if value > MAX_INITIAL_WINDOW_SIZE {
                        return Err(H2ErrorCode::FlowControlError);
                    }
                    settings.initial_window_size = Some(value);
                }
                SETTINGS_MAX_FRAME_SIZE => {
                    if !(MIN_MAX_FRAME_SIZE..=MAX_MAX_FRAME_SIZE).contains(&value) {
                        return Err(H2ErrorCode::ProtocolError);
                    }
                    settings.max_frame_size = Some(value);
                }
                SETTINGS_MAX_HEADER_LIST_SIZE => settings.max_header_list_size = Some(value),
                _ => log::trace!("skipping unknown setting identifier {id:#x}"),
            }
        }
        Ok(settings)
    }

    /// Encode these settings into `buf`. Returns the number of bytes written, or `None` if the
    /// buffer is too small (check [`encoded_len`](Self::encoded_len) first).
    pub(crate) fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        if buf.len() < self.encoded_len() {
            return None;
        }
        let mut written = 0;
        if let Some(v) = self.header_table_size {
            written += write_entry(SETTINGS_HEADER_TABLE_SIZE, v, &mut buf[written..]);
        }
        if let Some(b) = self.enable_push {
            written += write_entry(SETTINGS_ENABLE_PUSH, u32::from(b), &mut buf[written..]);
        }
        if let Some(v) = self.max_concurrent_streams {
            written += write_entry(SETTINGS_MAX_CONCURRENT_STREAMS, v, &mut buf[written..]);
        }
        if let Some(v) = self.initial_window_size {
            written += write_entry(SETTINGS_INITIAL_WINDOW_SIZE, v, &mut buf[written..]);
        }
        if let Some(v) = self.max_frame_size {
            written += write_entry(SETTINGS_MAX_FRAME_SIZE, v, &mut buf[written..]);
        }
        if let Some(v) = self.max_header_list_size {
            written += write_entry(SETTINGS_MAX_HEADER_LIST_SIZE, v, &mut buf[written..]);
        }
        if self.grease_id != 0 {
            written += write_entry(self.grease_id, self.grease_value, &mut buf[written..]);
        }
        Some(written)
    }
}

fn write_entry(id: u16, value: u32, buf: &mut [u8]) -> usize {
    buf[0..2].copy_from_slice(&id.to_be_bytes());
    buf[2..6].copy_from_slice(&value.to_be_bytes());
    SETTING_ENTRY_LEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_fields() {
        let settings = H2Settings::server_defaults()
            .with_header_table_size(4096)
            .with_initial_window_size(65_536)
            .with_max_frame_size(32_768);
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        assert_eq!(len, settings.encoded_len());
        let decoded = H2Settings::decode(&buf[..len]).unwrap();
        assert_eq!(decoded, settings);
    }

    #[test]
    fn roundtrip_empty_still_grease() {
        // server_defaults always has GREASE
        let settings = H2Settings::server_defaults();
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        assert!(len > 0);
        let decoded = H2Settings::decode(&buf[..len]).unwrap();
        assert_eq!(decoded, settings);
    }

    #[test]
    fn decode_default_empty_payload() {
        let settings = H2Settings::decode(&[]).unwrap();
        assert_eq!(settings, H2Settings::default());
    }

    #[test]
    fn odd_payload_length_is_frame_size_error() {
        // 5 bytes — not a multiple of 6
        assert_eq!(
            H2Settings::decode(&[0, 1, 0, 0, 0]),
            Err(H2ErrorCode::FrameSizeError),
        );
    }

    #[test]
    fn invalid_enable_push_is_protocol_error() {
        let mut payload = [0; 6];
        write_entry(SETTINGS_ENABLE_PUSH, 2, &mut payload);
        assert_eq!(
            H2Settings::decode(&payload),
            Err(H2ErrorCode::ProtocolError),
        );
    }

    #[test]
    fn enable_push_zero_and_one_both_ok() {
        for value in [0u32, 1] {
            let mut payload = [0; 6];
            write_entry(SETTINGS_ENABLE_PUSH, value, &mut payload);
            let decoded = H2Settings::decode(&payload).unwrap();
            assert_eq!(decoded.enable_push, Some(value == 1));
        }
    }

    #[test]
    fn oversized_initial_window_is_flow_control_error() {
        let mut payload = [0; 6];
        write_entry(SETTINGS_INITIAL_WINDOW_SIZE, 1 << 31, &mut payload);
        assert_eq!(
            H2Settings::decode(&payload),
            Err(H2ErrorCode::FlowControlError),
        );
    }

    #[test]
    fn initial_window_at_max_is_allowed() {
        let mut payload = [0; 6];
        write_entry(
            SETTINGS_INITIAL_WINDOW_SIZE,
            MAX_INITIAL_WINDOW_SIZE,
            &mut payload,
        );
        let decoded = H2Settings::decode(&payload).unwrap();
        assert_eq!(decoded.initial_window_size, Some(MAX_INITIAL_WINDOW_SIZE));
    }

    #[test]
    fn undersized_max_frame_size_is_protocol_error() {
        let mut payload = [0; 6];
        write_entry(SETTINGS_MAX_FRAME_SIZE, 16_383, &mut payload);
        assert_eq!(
            H2Settings::decode(&payload),
            Err(H2ErrorCode::ProtocolError),
        );
    }

    #[test]
    fn oversized_max_frame_size_is_protocol_error() {
        let mut payload = [0; 6];
        write_entry(SETTINGS_MAX_FRAME_SIZE, 1 << 24, &mut payload);
        assert_eq!(
            H2Settings::decode(&payload),
            Err(H2ErrorCode::ProtocolError),
        );
    }

    #[test]
    fn max_frame_size_at_bounds_allowed() {
        for value in [MIN_MAX_FRAME_SIZE, MAX_MAX_FRAME_SIZE] {
            let mut payload = [0; 6];
            write_entry(SETTINGS_MAX_FRAME_SIZE, value, &mut payload);
            let decoded = H2Settings::decode(&payload).unwrap();
            assert_eq!(decoded.max_frame_size, Some(value));
        }
    }

    #[test]
    fn unknown_identifiers_are_skipped() {
        // A made-up setting ID 0x9999 with value 42, followed by MAX_CONCURRENT_STREAMS=50.
        let mut payload = [0; 12];
        write_entry(0x9999, 42, &mut payload[..6]);
        write_entry(SETTINGS_MAX_CONCURRENT_STREAMS, 50, &mut payload[6..]);
        let decoded = H2Settings::decode(&payload).unwrap();
        assert_eq!(decoded.max_concurrent_streams, Some(50));
    }

    #[test]
    fn grease_id_matches_rfc_8701_pattern() {
        // Every GREASE id must match NaNa for n in 0..=15.
        for _ in 0..100 {
            let settings = H2Settings::server_defaults();
            let id = settings.grease_id;
            let n = (id >> 12) & 0xf;
            assert_eq!(id, 0x0A0A | (n << 12) | (n << 4), "bad GREASE id {id:#x}");
        }
    }

    #[test]
    fn grease_skipped_on_decode() {
        let settings = H2Settings::server_defaults();
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        let decoded = H2Settings::decode(&buf[..len]).unwrap();
        // decoded has no grease id
        assert_eq!(decoded.grease_id, 0);
    }

    #[test]
    fn later_setting_overrides_earlier() {
        let mut payload = [0; 12];
        write_entry(SETTINGS_MAX_CONCURRENT_STREAMS, 10, &mut payload[..6]);
        write_entry(SETTINGS_MAX_CONCURRENT_STREAMS, 100, &mut payload[6..]);
        let decoded = H2Settings::decode(&payload).unwrap();
        assert_eq!(decoded.max_concurrent_streams, Some(100));
    }
}
