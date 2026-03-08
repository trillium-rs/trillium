use super::quic_varint;
use crate::HttpConfig;
use fieldwork::Fieldwork;

/// H3 settings identifiers that are forbidden because they belong to HTTP/2.
/// Receiving any of these is a connection error of type `H3_SETTINGS_ERROR`
/// per RFC 9114 §7.2.4.1.
const FORBIDDEN_H2_SETTINGS: &[u64] = &[0x00, 0x02, 0x03, 0x04, 0x05];

/// Known H3 setting identifiers.
const SETTINGS_QPACK_MAX_TABLE_CAPACITY: u64 = 0x01;
const SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
const SETTINGS_QPACK_BLOCKED_STREAMS: u64 = 0x07;
/// RFC 9297 §2.1 — enables QUIC DATAGRAM frames for HTTP/3.
const SETTINGS_H3_DATAGRAM: u64 = 0x33;
/// draft-ietf-webtrans-http3 — enables WebTransport over HTTP/3.
const SETTINGS_ENABLE_WEBTRANSPORT: u64 = 0x2b60_3742;

/// H3 connection settings per RFC 9114 §7.2.4.
///
/// Sent once at the beginning of each control stream. `None` fields
/// mean the setting was absent, implying the default value
/// (unlimited for `max_field_section_size`, 0 for the QPACK settings).
///
/// Use [`H3Settings::new`] to create outgoing settings (generates GREASE
/// values). [`H3Settings::decode`] is used for incoming settings.
#[derive(Clone, Copy, Eq, Fieldwork, Default)]
#[fieldwork(get, set, with(option_set_some))]
pub struct H3Settings {
    /// The maximum size of a field section (header block) the peer may send
    ///
    /// Default: unlimited.
    #[field(copy)]
    max_field_section_size: Option<u64>,

    /// The maximum capacity of the QPACK dynamic table
    ///
    /// Default: 0.
    #[field(copy)]
    qpack_max_table_capacity: Option<u64>,

    /// The maximum number of streams that can be blocked on QPACK
    ///
    /// Default: 0.
    #[field(copy)]
    qpack_blocked_streams: Option<u64>,

    /// Whether QUIC DATAGRAM frames are enabled for HTTP/3 (RFC 9297 §2.1).
    ///
    /// Default: false (disabled).
    h3_datagram: bool,

    /// Whether WebTransport is enabled (draft-ietf-webtrans-http3).
    ///
    /// Default: false (disabled).
    enable_webtransport: bool,

    // GREASE setting included in encoded output per RFC 9114 §7.2.4.1.
    // Chosen at construction time so encoded_len() and encode() agree.
    // Zero when decoded from a peer (GREASE is skipped during decode).
    #[field = false]
    grease_id: u64,

    #[field = false]
    grease_value: u64,
}

impl std::fmt::Debug for H3Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H3Settings")
            .field("max_field_section_size", &self.max_field_section_size)
            .field("qpack_max_table_capacity", &self.qpack_max_table_capacity)
            .field("qpack_blocked_streams", &self.qpack_blocked_streams)
            .field("h3_datagram", &self.h3_datagram)
            .field("enable_webtransport", &self.enable_webtransport)
            .finish_non_exhaustive()
    }
}

impl From<&HttpConfig> for H3Settings {
    fn from(value: &HttpConfig) -> Self {
        Self {
            max_field_section_size: value.h3_max_field_section_size,
            enable_webtransport: value.webtransport_enabled,
            h3_datagram: value.h3_datagrams_enabled,
            ..Self::default()
        }
    }
}

/// `PartialEq` ignores GREASE values — they're an encoding detail,
/// not semantically meaningful.
impl PartialEq for H3Settings {
    fn eq(&self, other: &Self) -> bool {
        self.max_field_section_size == other.max_field_section_size
            && self.qpack_max_table_capacity == other.qpack_max_table_capacity
            && self.qpack_blocked_streams == other.qpack_blocked_streams
            && self.h3_datagram == other.h3_datagram
            && self.enable_webtransport == other.enable_webtransport
    }
}

impl H3Settings {
    /// Create a new settings struct for sending to a peer.
    ///
    /// Generates random GREASE values per RFC 9114 §7.2.4.1.
    pub fn new() -> Self {
        let n = u64::from(fastrand::u16(..));
        Self {
            grease_id: 0x1f * n + 0x21,
            grease_value: u64::from(fastrand::u32(..)),
            ..Default::default()
        }
    }

    /// The number of bytes this settings payload will occupy when encoded.
    pub fn encoded_len(&self) -> usize {
        let mut len = 0;
        if let Some(v) = self.qpack_max_table_capacity {
            len += quic_varint::encoded_len(SETTINGS_QPACK_MAX_TABLE_CAPACITY);
            len += quic_varint::encoded_len(v);
        }
        if let Some(v) = self.max_field_section_size {
            len += quic_varint::encoded_len(SETTINGS_MAX_FIELD_SECTION_SIZE);
            len += quic_varint::encoded_len(v);
        }
        if let Some(v) = self.qpack_blocked_streams {
            len += quic_varint::encoded_len(SETTINGS_QPACK_BLOCKED_STREAMS);
            len += quic_varint::encoded_len(v);
        }
        if self.h3_datagram {
            len += quic_varint::encoded_len(SETTINGS_H3_DATAGRAM);
            len += quic_varint::encoded_len(1u64);
        }
        if self.enable_webtransport {
            len += quic_varint::encoded_len(SETTINGS_ENABLE_WEBTRANSPORT);
            len += quic_varint::encoded_len(1u64);
        }
        if self.grease_id != 0 {
            len += quic_varint::encoded_len(self.grease_id);
            len += quic_varint::encoded_len(self.grease_value);
        }
        len
    }

    /// Decode settings from a SETTINGS frame payload.
    ///
    /// Returns `None` if a forbidden H2 setting identifier
    /// is encountered. Unknown identifiers (including GREASE) are skipped.
    pub fn decode(payload: &[u8]) -> Option<Self> {
        let mut settings = Self::default();
        let mut bytes_read = 0;
        while bytes_read < payload.len() {
            let (id, br) = quic_varint::decode(&payload[bytes_read..]).ok()?;
            bytes_read += br;

            let (value, br) = quic_varint::decode(&payload[bytes_read..]).ok()?;
            bytes_read += br;

            if FORBIDDEN_H2_SETTINGS.contains(&id) {
                log::trace!("received forbidden H2 setting identifier {id:#x}");
                return None;
            }

            match id {
                SETTINGS_QPACK_MAX_TABLE_CAPACITY => {
                    settings.qpack_max_table_capacity = Some(value);
                }
                SETTINGS_MAX_FIELD_SECTION_SIZE => {
                    settings.max_field_section_size = Some(value);
                }
                SETTINGS_QPACK_BLOCKED_STREAMS => {
                    settings.qpack_blocked_streams = Some(value);
                }
                SETTINGS_H3_DATAGRAM => {
                    settings.h3_datagram = value == 1;
                }
                SETTINGS_ENABLE_WEBTRANSPORT => {
                    settings.enable_webtransport = value == 1;
                }
                _ => {
                    log::trace!("skipping unknown setting identifier {id:#x}");
                }
            }
        }
        Some(settings)
    }

    /// Encode these settings into a byte slice. Returns bytes written or None if unable to fit.
    ///
    /// Panics if `buf` is too small (check [`encoded_len`](Self::encoded_len)).
    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        let mut written = 0;
        if let Some(v) = self.qpack_max_table_capacity {
            written += quic_varint::encode(SETTINGS_QPACK_MAX_TABLE_CAPACITY, &mut buf[written..])?;
            written += quic_varint::encode(v, &mut buf[written..])?;
        }
        if let Some(v) = self.max_field_section_size {
            written += quic_varint::encode(SETTINGS_MAX_FIELD_SECTION_SIZE, &mut buf[written..])?;
            written += quic_varint::encode(v, &mut buf[written..])?;
        }
        if let Some(v) = self.qpack_blocked_streams {
            written += quic_varint::encode(SETTINGS_QPACK_BLOCKED_STREAMS, &mut buf[written..])?;
            written += quic_varint::encode(v, &mut buf[written..])?;
        }
        if self.h3_datagram {
            written += quic_varint::encode(SETTINGS_H3_DATAGRAM, &mut buf[written..])?;
            written += quic_varint::encode(1u64, &mut buf[written..])?;
        }
        if self.enable_webtransport {
            written += quic_varint::encode(SETTINGS_ENABLE_WEBTRANSPORT, &mut buf[written..])?;
            written += quic_varint::encode(1u64, &mut buf[written..])?;
        }
        if self.grease_id != 0 {
            written += quic_varint::encode(self.grease_id, &mut buf[written..])?;
            written += quic_varint::encode(self.grease_value, &mut buf[written..])?;
        }
        Some(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_fields() {
        let settings = H3Settings::new()
            .with_max_field_section_size(8192)
            .with_qpack_max_table_capacity(4096)
            .with_qpack_blocked_streams(100);
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        let decoded = H3Settings::decode(&buf[..len]).unwrap();
        assert_eq!(decoded, settings);
    }

    #[test]
    fn roundtrip_partial() {
        let settings = H3Settings::new().with_max_field_section_size(1024);
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        let decoded = H3Settings::decode(&buf[..len]).unwrap();
        assert_eq!(decoded, settings);
    }

    #[test]
    fn roundtrip_empty() {
        let settings = H3Settings::new();
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        // buf is not empty — it has the GREASE setting
        assert!(len > 0);
        let decoded = H3Settings::decode(&buf[..len]).unwrap();
        // GREASE is skipped, so we get back defaults
        assert_eq!(decoded, settings);
    }

    #[test]
    fn encoded_len_matches_encode() {
        let settings = H3Settings::new()
            .with_max_field_section_size(8192)
            .with_qpack_max_table_capacity(4096)
            .with_qpack_blocked_streams(100);
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        assert_eq!(settings.encoded_len(), len);
    }

    #[test]
    fn encoded_len_empty() {
        let settings = H3Settings::new();
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        assert_eq!(settings.encoded_len(), len);
    }

    #[test]
    fn roundtrip_webtransport_settings() {
        let settings = H3Settings::new()
            .with_h3_datagram(true)
            .with_enable_webtransport(true);
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        assert_eq!(settings.encoded_len(), len);
        let decoded = H3Settings::decode(&buf[..len]).unwrap();
        assert_eq!(decoded, settings);
        assert!(decoded.h3_datagram());
        assert!(decoded.enable_webtransport());
    }

    #[test]
    fn webtransport_settings_default_false() {
        let settings = H3Settings::default();
        assert!(!settings.h3_datagram());
        assert!(!settings.enable_webtransport());

        // When false, they shouldn't appear on the wire (only GREASE from new())
        let settings = H3Settings::new();
        let mut buf = vec![0; 256];
        let len = settings.encode(&mut buf).unwrap();
        let decoded = H3Settings::decode(&buf[..len]).unwrap();
        assert!(!decoded.h3_datagram());
        assert!(!decoded.enable_webtransport());
    }

    #[test]
    fn forbidden_h2_setting() {
        for &id in FORBIDDEN_H2_SETTINGS {
            let mut buf = vec![0; 256];
            let mut written = 0;
            written += quic_varint::encode(id, &mut buf[written..]).unwrap();
            written += quic_varint::encode(0u64, &mut buf[written..]).unwrap();
            assert_eq!(H3Settings::decode(&buf[..written]), None);
        }
    }

    #[test]
    fn truncated_payload() {
        // A single byte that starts a 2-byte varint — incomplete
        assert_eq!(H3Settings::decode(&[0x40]), None);
    }

    #[test]
    fn id_present_but_value_missing() {
        let mut buf = vec![0; 256];
        let written = quic_varint::encode(SETTINGS_MAX_FIELD_SECTION_SIZE, &mut buf).unwrap();
        // no value follows
        assert_eq!(H3Settings::decode(&buf[..written]), None);
    }
}
