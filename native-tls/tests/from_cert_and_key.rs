//! End-to-end tests for [`NativeTlsAcceptor::from_cert_and_key`] across all
//! key formats and platform backends. Internally the constructor tries
//! [`Identity::from_pkcs8`] first and falls back to building an in-memory
//! PKCS#12 archive when that fails — handling the macOS Secure Transport
//! case where EC keys are refused via the PKCS#8 PEM path.
//!
//! The EC tests are skipped on Windows: SChannel refuses our EC PKCS#8 PEM
//! imports (`ASN1 bad tag`) AND the `p12`-crate-built fallback archive
//! (which omits the `LocalKeyId` attribute SChannel uses to pair cert ↔
//! key). Windows EC users should reach for `trillium-rustls` or supply a
//! pre-built PKCS#12 archive via [`NativeTlsAcceptor::from_pkcs12`].

use trillium_native_tls::NativeTlsAcceptor;

const RSA_CERT: &[u8] = include_bytes!("fixtures/rsa.crt");
const RSA_PKCS8: &[u8] = include_bytes!("fixtures/rsa-pkcs8.key");
const RSA_PKCS1: &[u8] = include_bytes!("fixtures/rsa-pkcs1.key");

#[cfg(not(target_os = "windows"))]
const EC_CERT: &[u8] = include_bytes!("fixtures/ec.crt");
#[cfg(not(target_os = "windows"))]
const EC_PKCS8: &[u8] = include_bytes!("fixtures/ec-pkcs8.key");
#[cfg(not(target_os = "windows"))]
const EC_SEC1: &[u8] = include_bytes!("fixtures/ec-sec1.key");
#[cfg(not(target_os = "windows"))]
const EC_CHAIN_CERT: &[u8] = include_bytes!("fixtures/ec-chain.crt");
#[cfg(not(target_os = "windows"))]
const EC_CHAIN_KEY: &[u8] = include_bytes!("fixtures/ec-chain.key");

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

#[cfg(not(target_os = "windows"))]
#[test]
fn ec_pkcs8_pem() {
    NativeTlsAcceptor::from_cert_and_key(EC_CERT, EC_PKCS8);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn ec_sec1_pem() {
    NativeTlsAcceptor::from_cert_and_key(EC_CERT, EC_SEC1);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn ec_chain_with_intermediate() {
    // Two-certificate chain (leaf + signing CA) with a SEC1 EC key — the same
    // shape ACME-issued certs (e.g. tailnet, Let's Encrypt) typically take.
    NativeTlsAcceptor::from_cert_and_key(EC_CHAIN_CERT, EC_CHAIN_KEY);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn ec_concatenated_bundle() {
    let mut bundle = Vec::new();
    bundle.extend_from_slice(EC_CHAIN_CERT);
    bundle.extend_from_slice(EC_CHAIN_KEY);
    NativeTlsAcceptor::from_cert_and_key(&bundle, &bundle);
}
