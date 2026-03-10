#[cfg(feature = "aws-lc-rs")]
pub(crate) fn crypto_provider() -> std::sync::Arc<rustls::crypto::CryptoProvider> {
    #[cfg(any(feature = "ring", feature = "custom-crypto-provider"))]
    log::error!("multiple crypto provider features enabled, choosing aws-lc-rs");

    rustls::crypto::aws_lc_rs::default_provider().into()
}

#[cfg(all(not(feature = "aws-lc-rs"), feature = "ring"))]
pub(crate) fn crypto_provider() -> std::sync::Arc<rustls::crypto::CryptoProvider> {
    #[cfg(feature = "custom-crypto-provider")]
    log::error!("multiple crypto provider features enabled, choosing ring");

    rustls::crypto::ring::default_provider().into()
}

#[cfg(all(
    not(any(feature = "aws-lc-rs", feature = "ring")),
    feature = "custom-crypto-provider"
))]
pub(crate) fn crypto_provider() -> std::sync::Arc<rustls::crypto::CryptoProvider> {
    rustls::crypto::CryptoProvider::get_default()
        .expect(concat!(
            "`custom-crypto-provider` feature was enabled, but no default crypto ",
            "provider was found. Either configure a `rustls::ServerConfig::builder_with_provider` ",
            "and pass it to `trillium_quinn::QuicConfig::from_server_tls_config`, or use ",
            "`rustls::crypto::CryptoProvider::install_default` before building the config."
        ))
        .clone()
}

#[cfg(not(any(
    feature = "ring",
    feature = "aws-lc-rs",
    feature = "custom-crypto-provider"
)))]
pub(crate) fn crypto_provider() -> std::sync::Arc<rustls::crypto::CryptoProvider> {
    compile_error!(
        "\n\n`trillium-quinn` cannot compile without a crypto provider feature enabled.
Please enable `ring`, `aws-lc-rs`, or `custom-crypto-provider`.

To use ring or aws-lc-rs, nothing further is needed than enabling the feature.
To use `custom-crypto-provider`, either configure a `rustls::ServerConfig::builder_with_provider` \
        and pass it to `trillium_quinn::QuicConfig::from_server_tls_config`, or use \
        `rustls::crypto::CryptoProvider::install_default` before building the config.\n\n"
    )
}
