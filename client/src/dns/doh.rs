//! DNS-over-HTTPS ([RFC 8484]): queries are POSTed as `application/dns-message` bodies over the
//! client's own connection pool, so DoH inherits the client's runtime, TLS stack, and HTTP/1–3
//! negotiation with no resolver IO of its own.
//!
//! [RFC 8484]: https://www.rfc-editor.org/rfc/rfc8484

use crate::Client;
use std::io;
use trillium_http::{KnownHeaderName, Version};
use trillium_server_common::url::Url;

/// The RFC 8484 media type for both the DoH request and response body.
const DNS_MESSAGE: &str = "application/dns-message";

/// DoH transport: the resolver endpoint URL plus an optional HTTP-version pin for the connection
/// to it.
#[derive(Debug, Clone)]
pub(super) struct Doh {
    resolver: Url,
    /// Optional pinned HTTP version for the connection *to the resolver*. `None`
    /// negotiates normally (h1/h2 via ALPN). Set to `Http3` to force DoH-over-h3
    /// for resolvers that speak it but don't advertise Alt-Svc.
    http_version: Option<Version>,
}

impl Doh {
    pub(super) fn new(resolver: Url, http_version: Option<Version>) -> Self {
        Self {
            resolver,
            http_version,
        }
    }

    pub(super) fn host(&self) -> Option<&str> {
        self.resolver.host_str()
    }

    pub(super) fn resolver(&self) -> &Url {
        &self.resolver
    }

    /// POST a single wire-format query to the resolver and return the raw response body.
    pub(super) async fn exchange(&self, client: &Client, query: Vec<u8>) -> io::Result<Vec<u8>> {
        let mut request = client
            .post(self.resolver.as_str())
            .with_request_header(KnownHeaderName::ContentType, DNS_MESSAGE)
            .with_request_header(KnownHeaderName::Accept, DNS_MESSAGE)
            .with_body(query);

        if let Some(version) = self.http_version {
            request = request.with_http_version(version);
        }

        let mut conn = request.await.map_err(io::Error::other)?;
        log::debug!(
            "DoH query to {} over {:?}",
            self.resolver,
            conn.http_version()
        );

        conn.response_body()
            .read_bytes()
            .await
            .map_err(io::Error::other)
    }
}
