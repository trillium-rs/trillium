//! End-to-end tests for [`NativeTlsAcceptor::from_cert_and_key`].
//!
//! Only the RSA cases run through the full native-tls `Identity` build, because
//! macOS Secure Transport (the backend used in CI on darwin) refuses to import
//! EC PKCS#8 PEM keys at all — that's a platform limitation independent of our
//! normalization. EC wrapping correctness is covered by unit tests in
//! `src/server.rs` that validate the produced PKCS#8 structurally.

use trillium_native_tls::NativeTlsAcceptor;

const RSA_CERT: &[u8] = include_bytes!("fixtures/rsa.crt");
const RSA_PKCS8: &[u8] = include_bytes!("fixtures/rsa-pkcs8.key");
const RSA_PKCS1: &[u8] = include_bytes!("fixtures/rsa-pkcs1.key");

#[test]
fn rsa_pkcs8_pem() {
    NativeTlsAcceptor::from_cert_and_key(RSA_CERT, RSA_PKCS8);
}

#[test]
fn rsa_pkcs1_pem() {
    NativeTlsAcceptor::from_cert_and_key(RSA_CERT, RSA_PKCS1);
}

#[test]
fn rsa_concatenated_bundle() {
    let mut bundle = Vec::new();
    bundle.extend_from_slice(RSA_CERT);
    bundle.extend_from_slice(RSA_PKCS1);
    NativeTlsAcceptor::from_cert_and_key(&bundle, &bundle);
}
