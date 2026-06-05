use crate::Identity;
use async_native_tls::{Error, TlsAcceptor, TlsStream};
use pem::Pem;
use pkcs8::{
    AlgorithmIdentifierRef, ObjectIdentifier, PrivateKeyInfoRef,
    der::{
        Decode, Encode,
        asn1::{AnyRef, OctetStringRef},
    },
};
use std::{
    io::{self, IoSlice, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_server_common::{Acceptor, AsyncRead, AsyncWrite, Transport};

/// trillium [`Acceptor`] for native-tls

#[derive(Clone, Debug)]
pub struct NativeTlsAcceptor(TlsAcceptor);

impl NativeTlsAcceptor {
    /// constructs a NativeTlsAcceptor from a [`native_tls::TlsAcceptor`],
    /// an [`async_native_tls::TlsAcceptor`], or an [`Identity`]
    pub fn new(t: impl Into<Self>) -> Self {
        t.into()
    }

    /// Construct a `NativeTlsAcceptor` from a PEM-encoded certificate chain
    /// and a PEM-encoded private key.
    ///
    /// This is the recommended entrypoint and matches the input format used by
    /// `trillium-rustls` and `trillium-openssl`. The cert input may contain one
    /// or more `CERTIFICATE` blocks (the leaf followed by any intermediates).
    /// The key input is accepted in any of the three common PEM key forms:
    ///
    /// - `-----BEGIN PRIVATE KEY-----` (PKCS#8)
    /// - `-----BEGIN RSA PRIVATE KEY-----` (PKCS#1)
    /// - `-----BEGIN EC PRIVATE KEY-----` (SEC1)
    ///
    /// Either argument may also be a single concatenated bundle containing
    /// both the cert chain and the key; the relevant blocks are extracted from
    /// each input. Encrypted keys are not supported here — decrypt first or
    /// use [`Self::from_pkcs12`].
    ///
    /// Internally we first try [`Identity::from_pkcs8`] with the normalized
    /// PEM inputs; on backends that reject that import path (notably macOS
    /// Secure Transport, which refuses EC keys this way with
    /// `errSecUnknownFormat`), we fall back to packaging the cert chain and
    /// key into an in-memory PKCS#12 archive and calling
    /// [`Identity::from_pkcs12`]. The fallback only runs when the first
    /// attempt fails, so OpenSSL-backed platforms never hit it.
    ///
    /// **Windows + EC keys:** SChannel rejects EC keys via both paths — its
    /// PKCS#8 PEM import is strict, and our fallback archive omits the
    /// `LocalKeyId` attribute SChannel uses to pair cert and key. For EC
    /// keys on Windows, prefer `trillium-rustls`, or supply a pre-built
    /// PKCS#12 archive (e.g. from `openssl pkcs12 -export`) via
    /// [`Self::from_pkcs12`]. RSA keys work on Windows.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use trillium_native_tls::NativeTlsAcceptor;
    /// const CERT: &[u8] = include_bytes!("../tests/fixtures/rsa.crt");
    /// const KEY: &[u8] = include_bytes!("../tests/fixtures/rsa-pkcs8.key");
    /// let acceptor = NativeTlsAcceptor::from_cert_and_key(CERT, KEY);
    /// ```
    pub fn from_cert_and_key(cert: &[u8], key: &[u8]) -> Self {
        let cert_chain_der = extract_cert_chain_der(cert);
        let key_pkcs8_der = normalize_key_to_pkcs8_der(key);

        let cert_chain_pem = encode_cert_chain_pem(&cert_chain_der);
        let key_pkcs8_pem = encode_pkcs8_pem(&key_pkcs8_der);
        let pkcs8_err = match Identity::from_pkcs8(&cert_chain_pem, &key_pkcs8_pem) {
            Ok(identity) => return identity.into(),
            Err(e) => e,
        };

        let p12_der = build_pkcs12_der(&cert_chain_der, &key_pkcs8_der);
        match Identity::from_pkcs12(&p12_der, INTERNAL_P12_PASSWORD) {
            Ok(identity) => identity.into(),
            Err(p12_err) => panic!(
                "could not build Identity from provided cert and key.\n  from_pkcs8 error: \
                 {pkcs8_err}\n  from_pkcs12 fallback error: {p12_err}"
            ),
        }
    }

    /// Construct a `NativeTlsAcceptor` from a PKCS#12 archive and password.
    ///
    /// PKCS#12 (`.p12`/`.pfx`) bundles a certificate chain and a private key
    /// in a single password-protected archive. Prefer
    /// [`Self::from_cert_and_key`] when you have separate cert and key PEM
    /// files, which is by far the more common provisioning format.
    pub fn from_pkcs12(der: &[u8], password: &str) -> Self {
        Identity::from_pkcs12(der, password)
            .expect("could not build Identity from provided pkcs12 key and password")
            .into()
    }

    /// Construct a `NativeTlsAcceptor` directly from PKCS#8 PEM cert and key
    /// inputs, without normalization.
    ///
    /// Prefer [`Self::from_cert_and_key`], which accepts the same inputs plus
    /// PKCS#1 and SEC1 keys. This constructor is retained for backwards
    /// compatibility and forwards directly to [`Identity::from_pkcs8`].
    pub fn from_pkcs8(pem: &[u8], key: &[u8]) -> Self {
        Identity::from_pkcs8(pem, key)
            .expect("could not build Identity from provided pem and key")
            .into()
    }
}

const PEM_TAG_PKCS8: &str = "PRIVATE KEY";
const PEM_TAG_PKCS1: &str = "RSA PRIVATE KEY";
const PEM_TAG_SEC1: &str = "EC PRIVATE KEY";
const PEM_TAG_CERT: &str = "CERTIFICATE";

const RSA_ENCRYPTION_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const EC_PUBLIC_KEY_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");

// Password used for the in-memory PKCS#12 archive built by `from_cert_and_key`.
// The archive lives only inside this process, so the password is just a
// well-known value that lets us round-trip through the PKCS#12 import path.
const INTERNAL_P12_PASSWORD: &str = "trillium";

fn parse_pem_blocks(input: &[u8]) -> Vec<Pem> {
    pem::parse_many(input).expect("could not parse PEM input")
}

fn normalize_key_to_pkcs8_der(input: &[u8]) -> Vec<u8> {
    let blocks = parse_pem_blocks(input);
    let key = blocks
        .iter()
        .find(|b| matches!(b.tag(), PEM_TAG_PKCS8 | PEM_TAG_PKCS1 | PEM_TAG_SEC1))
        .expect(
            "no private key block found in key input (expected PRIVATE KEY, RSA PRIVATE KEY, or \
             EC PRIVATE KEY)",
        );

    match key.tag() {
        PEM_TAG_PKCS8 => key.contents().to_vec(),
        PEM_TAG_PKCS1 => wrap_pkcs1_in_pkcs8(key.contents()),
        PEM_TAG_SEC1 => wrap_sec1_in_pkcs8(key.contents()),
        _ => unreachable!(),
    }
}

fn wrap_pkcs1_in_pkcs8(pkcs1_der: &[u8]) -> Vec<u8> {
    let algorithm = AlgorithmIdentifierRef {
        oid: RSA_ENCRYPTION_OID,
        parameters: Some(AnyRef::NULL),
    };
    let private_key =
        OctetStringRef::new(pkcs1_der).expect("could not wrap PKCS#1 key as OCTET STRING");
    PrivateKeyInfoRef::new(algorithm, private_key)
        .to_der()
        .expect("could not encode PKCS#1 key as PKCS#8")
}

fn wrap_sec1_in_pkcs8(sec1_der: &[u8]) -> Vec<u8> {
    let parsed =
        sec1::EcPrivateKey::from_der(sec1_der).expect("could not parse SEC1 EC private key");
    let curve_oid = parsed
        .parameters
        .and_then(|p| p.named_curve())
        .expect("EC private key is missing namedCurve parameters");
    let curve_param: AnyRef<'_> = (&curve_oid).into();
    let algorithm = AlgorithmIdentifierRef {
        oid: EC_PUBLIC_KEY_OID,
        parameters: Some(curve_param),
    };
    let private_key =
        OctetStringRef::new(sec1_der).expect("could not wrap SEC1 key as OCTET STRING");
    PrivateKeyInfoRef::new(algorithm, private_key)
        .to_der()
        .expect("could not encode SEC1 key as PKCS#8")
}

fn encode_cert_chain_pem(cert_chain_der: &[Vec<u8>]) -> Vec<u8> {
    let blocks: Vec<Pem> = cert_chain_der
        .iter()
        .map(|d| Pem::new(PEM_TAG_CERT, d.clone()))
        .collect();
    pem::encode_many(&blocks).into_bytes()
}

fn encode_pkcs8_pem(key_pkcs8_der: &[u8]) -> Vec<u8> {
    pem::encode(&Pem::new(PEM_TAG_PKCS8, key_pkcs8_der.to_vec())).into_bytes()
}

fn extract_cert_chain_der(input: &[u8]) -> Vec<Vec<u8>> {
    let certs: Vec<Vec<u8>> = parse_pem_blocks(input)
        .into_iter()
        .filter(|b| b.tag() == PEM_TAG_CERT)
        .map(|b| b.into_contents())
        .collect();
    assert!(
        !certs.is_empty(),
        "no CERTIFICATE blocks found in cert input"
    );
    certs
}

fn build_pkcs12_der(cert_chain_der: &[Vec<u8>], key_pkcs8_der: &[u8]) -> Vec<u8> {
    let leaf = cert_chain_der.first().expect("cert chain was empty");
    let intermediates: Vec<&[u8]> = cert_chain_der.iter().skip(1).map(Vec::as_slice).collect();
    let pfx = p12::PFX::new_with_cas(
        leaf,
        key_pkcs8_der,
        &intermediates,
        INTERNAL_P12_PASSWORD,
        "",
    )
    .expect("could not build PKCS#12 archive from cert and key");
    pfx.to_der()
}

impl From<Identity> for NativeTlsAcceptor {
    fn from(i: Identity) -> Self {
        native_tls::TlsAcceptor::new(i).unwrap().into()
    }
}

impl From<native_tls::TlsAcceptor> for NativeTlsAcceptor {
    fn from(i: native_tls::TlsAcceptor) -> Self {
        Self(i.into())
    }
}

impl From<TlsAcceptor> for NativeTlsAcceptor {
    fn from(i: TlsAcceptor) -> Self {
        Self(i)
    }
}

impl From<(&[u8], &str)> for NativeTlsAcceptor {
    fn from(i: (&[u8], &str)) -> Self {
        Self::from_pkcs12(i.0, i.1)
    }
}

impl<Input> Acceptor<Input> for NativeTlsAcceptor
where
    Input: Transport,
{
    type Error = Error;
    type Output = NativeTlsServerTransport<Input>;

    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        self.0.accept(input).await.map(NativeTlsServerTransport)
    }
}

/// Server Tls Transport
///
/// A wrapper type around [`TlsStream`] that also implements [`Transport`]
#[derive(Debug)]
pub struct NativeTlsServerTransport<T>(TlsStream<T>);

impl<T: AsyncWrite + AsyncRead + Unpin> AsRef<T> for NativeTlsServerTransport<T> {
    fn as_ref(&self) -> &T {
        self.0.get_ref()
    }
}
impl<T: AsyncWrite + AsyncRead + Unpin> AsMut<T> for NativeTlsServerTransport<T> {
    fn as_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}

impl<T> AsRef<TlsStream<T>> for NativeTlsServerTransport<T> {
    fn as_ref(&self) -> &TlsStream<T> {
        &self.0
    }
}
impl<T> AsMut<TlsStream<T>> for NativeTlsServerTransport<T> {
    fn as_mut(&mut self) -> &mut TlsStream<T> {
        &mut self.0
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for NativeTlsServerTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read_vectored(cx, bufs)
    }
}

impl<T: AsyncWrite + AsyncRead + Unpin> AsyncWrite for NativeTlsServerTransport<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write_vectored(cx, bufs)
    }
}

impl<T: Transport> Transport for NativeTlsServerTransport<T> {
    fn peer_addr(&self) -> io::Result<Option<SocketAddr>> {
        self.0.get_ref().peer_addr()
    }

    // `negotiated_alpn` is left at the trait default (`None`). Server-side ALPN advertisement in
    // `native-tls` lives behind the `alpn-accept` cargo feature, which `async-native-tls` 0.6 does
    // not enable, and the wrapper's `TlsStream` does not expose `negotiated_alpn` either — so
    // `trillium-native-tls` cannot perform ALPN-based h2 dispatch today. Revisit once the upstream
    // wrapper grows the missing surface.
}

#[cfg(test)]
mod tests {
    use super::{
        EC_PUBLIC_KEY_OID, RSA_ENCRYPTION_OID, extract_cert_chain_der, normalize_key_to_pkcs8_der,
    };
    use pkcs8::PrivateKeyInfoRef;

    const RSA_CERT: &[u8] = include_bytes!("../tests/fixtures/rsa.crt");
    const RSA_PKCS1: &[u8] = include_bytes!("../tests/fixtures/rsa-pkcs1.key");
    const EC_CERT: &[u8] = include_bytes!("../tests/fixtures/ec.crt");
    const EC_SEC1: &[u8] = include_bytes!("../tests/fixtures/ec-sec1.key");
    const EC_PKCS8: &[u8] = include_bytes!("../tests/fixtures/ec-pkcs8.key");

    fn parse_pkcs8_der(der: &[u8]) -> PrivateKeyInfoRef<'_> {
        PrivateKeyInfoRef::try_from(der).expect("output not parseable as PKCS#8")
    }

    #[test]
    fn pkcs1_wraps_to_pkcs8_with_rsa_oid() {
        let der = normalize_key_to_pkcs8_der(RSA_PKCS1);
        assert_eq!(parse_pkcs8_der(&der).algorithm.oid, RSA_ENCRYPTION_OID);
    }

    #[test]
    fn sec1_wraps_to_pkcs8_with_ec_oid_and_curve_param() {
        let der = normalize_key_to_pkcs8_der(EC_SEC1);
        let pki = parse_pkcs8_der(&der);
        assert_eq!(pki.algorithm.oid, EC_PUBLIC_KEY_OID);
        assert!(
            pki.algorithm.parameters.is_some(),
            "EC PKCS#8 must carry namedCurve OID in algorithm parameters"
        );
    }

    #[test]
    fn pkcs8_pass_through_preserves_algorithm() {
        let der = normalize_key_to_pkcs8_der(EC_PKCS8);
        assert_eq!(parse_pkcs8_der(&der).algorithm.oid, EC_PUBLIC_KEY_OID);
    }

    #[test]
    fn cert_extracted_from_concatenated_bundle() {
        let mut bundle = Vec::new();
        bundle.extend_from_slice(EC_CERT);
        bundle.extend_from_slice(EC_SEC1);

        let extracted = extract_cert_chain_der(&bundle);
        let original: Vec<Vec<u8>> = pem::parse_many(EC_CERT)
            .unwrap()
            .into_iter()
            .map(pem::Pem::into_contents)
            .collect();
        assert_eq!(extracted, original);
    }

    #[test]
    fn key_extracted_from_concatenated_bundle() {
        let mut bundle = Vec::new();
        bundle.extend_from_slice(RSA_CERT);
        bundle.extend_from_slice(RSA_PKCS1);
        let der = normalize_key_to_pkcs8_der(&bundle);
        assert_eq!(parse_pkcs8_der(&der).algorithm.oid, RSA_ENCRYPTION_OID);
    }
}
