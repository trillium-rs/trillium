/// Encode a list of ALPN protocol identifiers into the wire format expected by openssl
/// (each protocol prefixed by its length as a single byte).
pub(crate) fn encode_alpn<P: AsRef<[u8]>>(protocols: &[P]) -> Vec<u8> {
    let mut wire = Vec::with_capacity(
        protocols
            .iter()
            .map(|p| p.as_ref().len() + 1)
            .sum::<usize>(),
    );
    for protocol in protocols {
        let protocol = protocol.as_ref();
        let len = u8::try_from(protocol.len())
            .expect("ALPN protocol identifiers must be at most 255 bytes");
        wire.push(len);
        wire.extend_from_slice(protocol);
    }
    wire
}
