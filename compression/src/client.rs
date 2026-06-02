//! Client-side compression middleware for [`trillium-client`][trillium_client].
//!
//! This module is gated behind the `client` feature flag. It provides [`Compression`], a
//! [`ClientHandler`] that handles content-coding on behalf of a
//! [`Client`][trillium_client::Client] in both directions:
//!
//! - **Responses (always):** advertises the codings it can decode via the `Accept-Encoding` request
//!   header (unless one is already set), and transparently decodes a `Content-Encoding` response
//!   body it understands, stripping the header so the caller reads plaintext.
//! - **Requests (opt-in):** when a request encoding is selected, compresses the request body and
//!   sets `Content-Encoding`. This is opt-in because there is no pre-request negotiation in HTTP —
//!   the caller asserts prior knowledge that the origin accepts a compressed body. Select it
//!   handler-wide with [`Compression::with_default_encoding`], or per request by setting a
//!   [`CompressionAlgorithm`] on the conn's state. The per-request signal overrides the default,
//!   and [`CompressionAlgorithm::Identity`] opts a single request out of a configured default.
//!
//! Brotli, gzip, and zstd are supported, matching the server-side handler.
//!
//! # Example
//!
//! ```
//! use trillium_client::Client;
//! use trillium_compression::client::Compression;
//! # use trillium_testing::client_config;
//!
//! // Decode responses; leave request bodies uncompressed.
//! let client = Client::new(client_config()).with_handler(Compression::new());
//! ```

use crate::{CompressionAlgorithm, Levels};
use futures_lite::io::BufReader;
use trillium_client::{
    ClientHandler, Conn, ConnExt,
    KnownHeaderName::{AcceptEncoding, ContentEncoding},
    Result,
};

/// The codings this handler advertises in `Accept-Encoding`, in preference order.
const ACCEPT_ENCODING: &str = "zstd, br, gzip";

/// A [`ClientHandler`] that advertises and decodes compressed responses, and optionally
/// compresses request bodies.
///
/// See the [module-level documentation][self] for behavior details.
#[derive(Clone, Copy, Debug, Default)]
pub struct Compression {
    default_encoding: Option<CompressionAlgorithm>,
}

impl Compression {
    /// Construct a new [`Compression`] handler that decodes responses but does not compress
    /// request bodies by default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compress request bodies with `encoding` by default.
    ///
    /// Use this when every request through this client targets an origin known to accept a
    /// compressed request body. A [`CompressionAlgorithm`] set on an individual conn's state
    /// overrides this default, including [`CompressionAlgorithm::Identity`] to send one request
    /// uncompressed.
    pub fn with_default_encoding(mut self, encoding: CompressionAlgorithm) -> Self {
        self.default_encoding = Some(encoding);
        self
    }
}

impl ClientHandler for Compression {
    async fn run(&self, conn: &mut Conn) -> Result<()> {
        conn.request_headers_mut()
            .try_insert(AcceptEncoding, ACCEPT_ENCODING);

        let Some(encoding) = conn
            .state::<CompressionAlgorithm>()
            .copied()
            .or(self.default_encoding)
        else {
            return Ok(());
        };

        if encoding == CompressionAlgorithm::Identity
            || conn.request_headers().get_str(ContentEncoding).is_some()
        {
            return Ok(());
        }

        let Some(body) = conn.take_request_body() else {
            return Ok(());
        };

        let (body, encoded) = encoding.encode(body, Levels::default()).await;
        conn.set_request_body(body);
        if encoded {
            conn.request_headers_mut()
                .insert(ContentEncoding, encoding.as_str());
        }

        Ok(())
    }

    async fn after_response(&self, conn: &mut Conn) -> Result<()> {
        let Some(encoding) = conn
            .response_headers()
            .get_str(ContentEncoding)
            .and_then(|encoding| encoding.parse::<CompressionAlgorithm>().ok())
            .filter(|&encoding| encoding != CompressionAlgorithm::Identity)
        else {
            return Ok(());
        };

        let Some(body) = conn.take_response_body() else {
            return Ok(());
        };

        let decoded = encoding.decode_streaming(BufReader::new(body));
        conn.set_response_body(decoded)
            .response_headers_mut()
            .remove(ContentEncoding);

        Ok(())
    }
}
