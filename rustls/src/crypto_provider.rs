use futures_rustls::rustls::crypto::CryptoProvider;
use std::sync::Arc;

#[cfg(feature = "aws-lc-rs")]
pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    #[cfg(any(feature = "ring", feature = "custom-crypto-provider"))]
    log::error!("multiple crypto provider features enabled, choosing aws-lc-rs");

    futures_rustls::rustls::crypto::aws_lc_rs::default_provider().into()
}

#[cfg(all(not(feature = "aws-lc-rs"), feature = "ring"))]
pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    #[cfg(feature = "custom-crypto-provider")]
    log::error!("multiple crypto provider features enabled, choosing ring");
    futures_rustls::rustls::crypto::ring::default_provider().into()
}

#[cfg(all(
    not(any(feature = "aws-lc-rs", feature = "ring")),
    feature = "custom-crypto-provider"
))]
pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    CryptoProvider::get_default()
        .expect(concat!(
            "`custom-crypto-provider` feature was enabled, but no default crypto ",
            "provider was found. Either configure a ClientConfig::builder_with_provider and pass",
            "it to `trillium_rustls::RustlsConfig::new` or use `CryptoProvider::install_default`."
        ))
        .clone()
}

#[cfg(not(any(
    feature = "ring",
    feature = "aws-lc-rs",
    feature = "custom-crypto-provider"
)))]
pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    compile_error!(
        "\n\n`trillium-rustls` cannot compile without a crypto provider feature enabled.
Please enable `ring`, `aws-lc-rs`, or `custom-crypto-provider`.

To use ring or aws-lc-rs, nothing further is needed than enabling the feature.
To use `custom-crypto-provider`, either configure a `ClientConfig::builder_with_provider` \
and pass it to `trillium_rustls::RustlsConfig::new` or use \
`CryptoProvider::install_default` before building the trillium_rustls::RustlsConfig::default().\n\n"
    )
}
