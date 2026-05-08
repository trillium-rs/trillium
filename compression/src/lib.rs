//! Body compression for trillium.rs
//!
//! Currently, this crate only supports compressing outbound bodies with
//! the zstd, brotli, and gzip algorithms (in order of preference),
//! although more algorithms may be added in the future. The correct
//! algorithm will be selected based on the Accept-Encoding header sent by
//! the client, if one exists.
//!
//! Defaults are tuned for HTTP transport: brotli at quality 4 (matching
//! nginx/caddy/Cloudflare). To opt into stronger or weaker compression,
//! see [`Compression::with_brotli_level`], [`Compression::with_gzip_level`],
//! and [`Compression::with_zstd_level`].
//!
//! Responses with `Content-Encoding` already set (e.g. precompressed
//! sidecars) are passed through unchanged. Responses with already-
//! compressed `Content-Type` (images, video, audio, fonts, archives) are
//! skipped by default.
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs)]

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use async_compression::futures::bufread::{BrotliEncoder, GzipEncoder, ZstdEncoder};
pub use async_compression::Level;
use futures_lite::{
    AsyncReadExt,
    io::{BufReader, Cursor},
};
use std::{
    collections::BTreeSet,
    fmt::{self, Display, Formatter},
    str::FromStr,
};
use trillium::{
    Body, Conn, Handler, HeaderValues,
    KnownHeaderName::{AcceptEncoding, ContentEncoding, ContentType, Vary},
    conn_try, conn_unwrap,
};

/// Algorithms supported by this crate
#[derive(PartialEq, Eq, Clone, Copy, Debug, Ord, PartialOrd)]
#[non_exhaustive]
pub enum CompressionAlgorithm {
    /// Brotli algorithm
    Brotli,

    /// Gzip algorithm
    Gzip,

    /// Zstd algorithm
    Zstd,
}

impl CompressionAlgorithm {
    fn as_str(&self) -> &'static str {
        match self {
            CompressionAlgorithm::Brotli => "br",
            CompressionAlgorithm::Gzip => "gzip",
            CompressionAlgorithm::Zstd => "zstd",
        }
    }

    fn from_str_exact(s: &str) -> Option<Self> {
        match s {
            "br" => Some(CompressionAlgorithm::Brotli),
            "gzip" => Some(CompressionAlgorithm::Gzip),
            "x-gzip" => Some(CompressionAlgorithm::Gzip),
            "zstd" => Some(CompressionAlgorithm::Zstd),
            _ => None,
        }
    }
}

impl AsRef<str> for CompressionAlgorithm {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CompressionAlgorithm {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_exact(s)
            .or_else(|| Self::from_str_exact(&s.to_ascii_lowercase()))
            .ok_or_else(|| format!("unrecognized coding {s}"))
    }
}

/// Trillium handler for compression
#[derive(Clone, Debug)]
pub struct Compression {
    algorithms: BTreeSet<CompressionAlgorithm>,
    brotli_level: Level,
    gzip_level: Level,
    zstd_level: Level,
}

impl Default for Compression {
    fn default() -> Self {
        use CompressionAlgorithm::*;
        Self {
            algorithms: [Zstd, Brotli, Gzip].into_iter().collect(),
            // q11 (async-compression default) is ~10x slower than q4 with
            // only a few percent better ratio — bad fit for the response
            // hot path. Match nginx/caddy transport defaults.
            brotli_level: Level::Precise(4),
            gzip_level: Level::Default,
            zstd_level: Level::Default,
        }
    }
}

impl Compression {
    /// constructs a new compression handler
    pub fn new() -> Self {
        Self::default()
    }

    fn set_algorithms(&mut self, algos: &[CompressionAlgorithm]) {
        self.algorithms = algos.iter().copied().collect();
    }

    /// sets the compression algorithms that this handler will
    /// use. the default of Zstd, Brotli, Gzip is recommended. Note that the
    /// order is ignored.
    pub fn with_algorithms(mut self, algorithms: &[CompressionAlgorithm]) -> Self {
        self.set_algorithms(algorithms);
        self
    }

    /// sets the brotli compression level. The default is `Level::Precise(4)`,
    /// matching common reverse proxy transport defaults. `Level::Default`
    /// resolves to brotli quality 11, which is much slower for marginal
    /// size gains.
    pub fn with_brotli_level(mut self, level: Level) -> Self {
        self.brotli_level = level;
        self
    }

    /// sets the gzip compression level. The default is `Level::Default`,
    /// which resolves to gzip level 6.
    pub fn with_gzip_level(mut self, level: Level) -> Self {
        self.gzip_level = level;
        self
    }

    /// sets the zstd compression level. The default is `Level::Default`,
    /// which resolves to zstd level 3.
    pub fn with_zstd_level(mut self, level: Level) -> Self {
        self.zstd_level = level;
        self
    }

    fn negotiate(&self, header: &str) -> Option<CompressionAlgorithm> {
        parse_accept_encoding(header)
            .into_iter()
            .find_map(|(algo, _)| {
                if self.algorithms.contains(&algo) {
                    Some(algo)
                } else {
                    None
                }
            })
    }
}

fn parse_accept_encoding(header: &str) -> Vec<(CompressionAlgorithm, u8)> {
    let mut vec = header
        .split(',')
        .filter_map(|s| {
            let mut iter = s.trim().split(';');
            let (algo, q) = (iter.next()?, iter.next());
            let algo = algo.trim().parse().ok()?;
            let q = q
                .and_then(|q| {
                    q.trim()
                        .strip_prefix("q=")
                        .and_then(|q| q.parse::<f32>().map(|f| (f * 100.0) as u8).ok())
                })
                .unwrap_or(100u8);
            Some((algo, q))
        })
        .collect::<Vec<(CompressionAlgorithm, u8)>>();

    vec.sort_by(|(algo_a, a), (algo_b, b)| match b.cmp(a) {
        std::cmp::Ordering::Equal => algo_a.cmp(algo_b),
        other => other,
    });

    vec
}

/// Returns true if the content-type identifies a payload that is already
/// compressed and should be passed through. The list covers image/audio/
/// video binary formats, web fonts, and common archive formats. Plain-
/// text-y types like `image/svg+xml` and `application/wasm` are intentionally
/// not skipped.
fn is_already_compressed(content_type: &str) -> bool {
    let primary = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    matches!(
        primary,
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "image/heic"
            | "image/heif"
            | "image/apng"
            | "image/x-icon"
            | "video/mp4"
            | "video/webm"
            | "video/ogg"
            | "video/quicktime"
            | "video/x-msvideo"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/webm"
            | "audio/aac"
            | "audio/flac"
            | "audio/mp4"
            | "font/woff"
            | "font/woff2"
            | "application/zip"
            | "application/gzip"
            | "application/x-gzip"
            | "application/x-bzip2"
            | "application/x-xz"
            | "application/x-7z-compressed"
            | "application/x-rar-compressed"
            | "application/zstd"
    ) || primary.starts_with("video/") || primary.starts_with("audio/")
}

impl Handler for Compression {
    async fn run(&self, mut conn: Conn) -> Conn {
        if let Some(header) = conn
            .request_headers()
            .get_str(AcceptEncoding)
            .and_then(|h| self.negotiate(h))
        {
            conn.insert_state(header);
        }
        conn
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        // Already encoded upstream (precompressed sidecar, or another
        // middleware ahead of us) — leave it alone.
        if conn.response_headers().get_str(ContentEncoding).is_some() {
            return conn;
        }

        // Skip already-compressed payloads (images, fonts, archives, ...).
        if conn
            .response_headers()
            .get_str(ContentType)
            .is_some_and(is_already_compressed)
        {
            return conn;
        }

        let Some(algo) = conn.state::<CompressionAlgorithm>().copied() else {
            return conn;
        };

        let mut body = conn_unwrap!(conn.take_response_body(), conn);
        let mut compression_used = false;

        if body.is_static() {
            let bytes = body.static_bytes().unwrap();
            let mut data = vec![];
            match algo {
                CompressionAlgorithm::Zstd => {
                    let mut encoder =
                        ZstdEncoder::with_quality(Cursor::new(bytes), self.zstd_level);
                    conn_try!(encoder.read_to_end(&mut data).await, conn);
                }
                CompressionAlgorithm::Brotli => {
                    let mut encoder =
                        BrotliEncoder::with_quality(Cursor::new(bytes), self.brotli_level);
                    conn_try!(encoder.read_to_end(&mut data).await, conn);
                }
                CompressionAlgorithm::Gzip => {
                    let mut encoder =
                        GzipEncoder::with_quality(Cursor::new(bytes), self.gzip_level);
                    conn_try!(encoder.read_to_end(&mut data).await, conn);
                }
            }
            if data.len() < bytes.len() {
                log::trace!(
                    "{} body from {} to {}",
                    algo.as_str(),
                    bytes.len(),
                    data.len()
                );
                compression_used = true;
                body = Body::new_static(data);
            }
        } else if body.is_streaming() {
            compression_used = true;
            match algo {
                CompressionAlgorithm::Zstd => {
                    body = Body::new_streaming(
                        ZstdEncoder::with_quality(BufReader::new(body.into_reader()), self.zstd_level),
                        None,
                    );
                }
                CompressionAlgorithm::Brotli => {
                    body = Body::new_streaming(
                        BrotliEncoder::with_quality(
                            BufReader::new(body.into_reader()),
                            self.brotli_level,
                        ),
                        None,
                    );
                }
                CompressionAlgorithm::Gzip => {
                    body = Body::new_streaming(
                        GzipEncoder::with_quality(BufReader::new(body.into_reader()), self.gzip_level),
                        None,
                    );
                }
            }
        }

        if compression_used {
            let vary = conn
                .response_headers()
                .get_str(Vary)
                .map(|vary| HeaderValues::from(format!("{vary}, Accept-Encoding")))
                .unwrap_or_else(|| HeaderValues::from("Accept-Encoding"));

            conn.response_headers_mut().extend([
                (ContentEncoding, HeaderValues::from(algo.as_str())),
                (Vary, vary),
            ]);
        }

        conn.with_body(body)
    }
}

/// Alias for [`Compression::new`](crate::Compression::new)
pub fn compression() -> Compression {
    Compression::new()
}
