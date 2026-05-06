use crate::Identity;
use async_native_tls::{Error, TlsAcceptor, TlsStream};
use pem::Pem;
use pkcs8::{
    AlgorithmIdentifierRef, ObjectIdentifier, PrivateKeyInfo,
    der::{Decode, Encode, asn1::AnyRef},
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
    /// The key input is accepted in any of the three common PEM key forms and
    /// is normalized to PKCS#8 before being handed to native-tls:
    ///
    /// - `-----BEGIN PRIVATE KEY-----` (PKCS#8) — passed through.
    /// - `-----BEGIN RSA PRIVATE KEY-----` (PKCS#1) — wrapped in a PKCS#8 envelope.
    /// - `-----BEGIN EC PRIVATE KEY-----` (SEC1) — wrapped in a PKCS#8 envelope.
    ///
    /// Either argument may also be a single concatenated bundle containing
    /// both the cert chain and the key; the relevant blocks are extracted from
    /// each input. Encrypted keys are not supported here — decrypt first or
    /// use [`Self::from_pkcs12`].
    ///
    /// Algorithm portability across native-tls backends (SChannel on Windows,
    /// Secure Transport on macOS, OpenSSL on Linux) is not guaranteed for all
    /// curves; RSA-2048+ and ECDSA P-256/P-384 are the safe choices.
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
        let cert_chain = extract_cert_chain_pem(cert);
        let key_pkcs8 = normalize_key_to_pkcs8_pem(key);
        Identity::from_pkcs8(&cert_chain, &key_pkcs8)
            .expect("could not build Identity from provided cert and key")
            .into()
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

fn parse_pem_blocks(input: &[u8]) -> Vec<Pem> {
    pem::parse_many(input).expect("could not parse PEM input")
}

fn normalize_key_to_pkcs8_pem(input: &[u8]) -> Vec<u8> {
    let blocks = parse_pem_blocks(input);
    let key = blocks
        .iter()
        .find(|b| matches!(b.tag(), PEM_TAG_PKCS8 | PEM_TAG_PKCS1 | PEM_TAG_SEC1))
        .expect(
            "no private key block found in key input (expected PRIVATE KEY, RSA PRIVATE KEY, or \
             EC PRIVATE KEY)",
        );

    let pkcs8_der = match key.tag() {
        PEM_TAG_PKCS8 => key.contents().to_vec(),
        PEM_TAG_PKCS1 => wrap_pkcs1_in_pkcs8(key.contents()),
        PEM_TAG_SEC1 => wrap_sec1_in_pkcs8(key.contents()),
        _ => unreachable!(),
    };

    pem::encode(&Pem::new(PEM_TAG_PKCS8, pkcs8_der)).into_bytes()
}

fn wrap_pkcs1_in_pkcs8(pkcs1_der: &[u8]) -> Vec<u8> {
    let algorithm = AlgorithmIdentifierRef {
        oid: RSA_ENCRYPTION_OID,
        parameters: Some(AnyRef::NULL),
    };
    PrivateKeyInfo::new(algorithm, pkcs1_der)
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
    PrivateKeyInfo::new(algorithm, sec1_der)
        .to_der()
        .expect("could not encode SEC1 key as PKCS#8")
}

fn extract_cert_chain_pem(input: &[u8]) -> Vec<u8> {
    let certs: Vec<Pem> = parse_pem_blocks(input)
        .into_iter()
        .filter(|b| b.tag() == PEM_TAG_CERT)
        .collect();
    assert!(
        !certs.is_empty(),
        "no CERTIFICATE blocks found in cert input"
    );
    pem::encode_many(&certs).into_bytes()
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
        EC_PUBLIC_KEY_OID, PEM_TAG_PKCS8, RSA_ENCRYPTION_OID, extract_cert_chain_pem,
        normalize_key_to_pkcs8_pem,
    };
    use pkcs8::PrivateKeyInfo;

    const RSA_CERT: &[u8] = include_bytes!("../tests/fixtures/rsa.crt");
    const RSA_PKCS1: &[u8] = include_bytes!("../tests/fixtures/rsa-pkcs1.key");
    const EC_CERT: &[u8] = include_bytes!("../tests/fixtures/ec.crt");
    const EC_SEC1: &[u8] = include_bytes!("../tests/fixtures/ec-sec1.key");
    const EC_PKCS8: &[u8] = include_bytes!("../tests/fixtures/ec-pkcs8.key");

    fn parse_pkcs8_pem(pem_bytes: &[u8]) -> PrivateKeyInfo<'_> {
        let block = pem::parse(pem_bytes).expect("output not parseable as PEM");
        assert_eq!(block.tag(), PEM_TAG_PKCS8, "wrong PEM armor label");
        let leaked: &'static [u8] = Box::leak(block.into_contents().into_boxed_slice());
        PrivateKeyInfo::try_from(leaked).expect("output not parseable as PKCS#8")
    }

    #[test]
    fn pkcs1_wraps_to_pkcs8_with_rsa_oid() {
        let pkcs8 = normalize_key_to_pkcs8_pem(RSA_PKCS1);
        let pki = parse_pkcs8_pem(&pkcs8);
        assert_eq!(pki.algorithm.oid, RSA_ENCRYPTION_OID);
    }

    #[test]
    fn sec1_wraps_to_pkcs8_with_ec_oid_and_curve_param() {
        let pkcs8 = normalize_key_to_pkcs8_pem(EC_SEC1);
        let pki = parse_pkcs8_pem(&pkcs8);
        assert_eq!(pki.algorithm.oid, EC_PUBLIC_KEY_OID);
        assert!(
            pki.algorithm.parameters.is_some(),
            "EC PKCS#8 must carry namedCurve OID in algorithm parameters"
        );
    }

    #[test]
    fn pkcs8_pass_through_preserves_algorithm() {
        let pkcs8 = normalize_key_to_pkcs8_pem(EC_PKCS8);
        let pki = parse_pkcs8_pem(&pkcs8);
        assert_eq!(pki.algorithm.oid, EC_PUBLIC_KEY_OID);
    }

    #[test]
    fn cert_extracted_from_concatenated_bundle() {
        let mut bundle = Vec::new();
        bundle.extend_from_slice(EC_CERT);
        bundle.extend_from_slice(EC_SEC1);

        let extracted_blocks = pem::parse_many(extract_cert_chain_pem(&bundle)).unwrap();
        let original_blocks = pem::parse_many(EC_CERT).unwrap();
        assert_eq!(extracted_blocks.len(), original_blocks.len());
        for (a, b) in extracted_blocks.iter().zip(original_blocks.iter()) {
            assert_eq!(a.tag(), "CERTIFICATE");
            assert_eq!(a.tag(), b.tag());
            assert_eq!(a.contents(), b.contents());
        }
    }

    #[test]
    fn key_extracted_from_concatenated_bundle() {
        let mut bundle = Vec::new();
        bundle.extend_from_slice(RSA_CERT);
        bundle.extend_from_slice(RSA_PKCS1);
        // Should not panic and should produce a valid PKCS#8 with RSA OID.
        let pkcs8 = normalize_key_to_pkcs8_pem(&bundle);
        let pki = parse_pkcs8_pem(&pkcs8);
        assert_eq!(pki.algorithm.oid, RSA_ENCRYPTION_OID);
    }
}
