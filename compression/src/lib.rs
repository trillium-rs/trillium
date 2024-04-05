/*!
Body compression for trillium.rs

Currently, this crate only supports compressing outbound bodies with
the zstd, brotli, and gzip algorithms (in order of preference),
although more algorithms may be added in the future. The correct
algorithm will be selected based on the Accept-Encoding header sent by
the client, if one exists.
*/
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs)]

use async_compression::futures::bufread::{BrotliEncoder, GzipEncoder, ZstdEncoder};
use futures_lite::{
    io::{BufReader, Cursor},
    AsyncReadExt,
};
use std::{
    collections::BTreeSet,
    fmt::{self, Display, Formatter},
    str::FromStr,
};
use trillium::{
    async_trait, conn_try, conn_unwrap, Body, Conn, Handler, HeaderValues,
    KnownHeaderName::{AcceptEncoding, ContentEncoding, Vary},
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

/**
Trillium handler for compression
*/
#[derive(Clone, Debug)]
pub struct Compression {
    algorithms: BTreeSet<CompressionAlgorithm>,
}

impl Default for Compression {
    fn default() -> Self {
        use CompressionAlgorithm::*;
        Self {
            algorithms: [Zstd, Brotli, Gzip].into_iter().collect(),
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

    /**
    sets the compression algorithms that this handler will
    use. the default of Zstd, Brotli, Gzip is recommended. Note that the
    order is ignored.
    */
    pub fn with_algorithms(mut self, algorithms: &[CompressionAlgorithm]) -> Self {
        self.set_algorithms(algorithms);
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

#[async_trait]
impl Handler for Compression {
    async fn run(&self, mut conn: Conn) -> Conn {
        if let Some(header) = conn
            .request_headers()
            .get_str(AcceptEncoding)
            .and_then(|h| self.negotiate(h))
        {
            conn.set_state(header);
        }
        conn
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(algo) = conn.state::<CompressionAlgorithm>().copied() {
            let mut body = conn_unwrap!(conn.inner_mut().take_response_body(), conn);
            let mut compression_used = false;

            if body.is_static() {
                match algo {
                    CompressionAlgorithm::Zstd => {
                        let bytes = body.static_bytes().unwrap();
                        let mut data = vec![];
                        let mut encoder = ZstdEncoder::new(Cursor::new(bytes));
                        conn_try!(encoder.read_to_end(&mut data).await, conn);
                        if data.len() < bytes.len() {
                            log::trace!("zstd body from {} to {}", bytes.len(), data.len());
                            compression_used = true;
                            body = Body::new_static(data);
                        }
                    }

                    CompressionAlgorithm::Brotli => {
                        let bytes = body.static_bytes().unwrap();
                        let mut data = vec![];
                        let mut encoder = BrotliEncoder::new(Cursor::new(bytes));
                        conn_try!(encoder.read_to_end(&mut data).await, conn);
                        if data.len() < bytes.len() {
                            log::trace!("brotli'd body from {} to {}", bytes.len(), data.len());
                            compression_used = true;
                            body = Body::new_static(data);
                        }
                    }

                    CompressionAlgorithm::Gzip => {
                        let bytes = body.static_bytes().unwrap();
                        let mut data = vec![];
                        let mut encoder = GzipEncoder::new(Cursor::new(bytes));
                        conn_try!(encoder.read_to_end(&mut data).await, conn);
                        if data.len() < bytes.len() {
                            log::trace!("gzipped body from {} to {}", bytes.len(), data.len());
                            body = Body::new_static(data);
                            compression_used = true;
                        }
                    }
                }
            } else if body.is_streaming() {
                compression_used = true;
                match algo {
                    CompressionAlgorithm::Zstd => {
                        body = Body::new_streaming(
                            ZstdEncoder::new(BufReader::new(body.into_reader())),
                            None,
                        );
                    }

                    CompressionAlgorithm::Brotli => {
                        body = Body::new_streaming(
                            BrotliEncoder::new(BufReader::new(body.into_reader())),
                            None,
                        );
                    }

                    CompressionAlgorithm::Gzip => {
                        body = Body::new_streaming(
                            GzipEncoder::new(BufReader::new(body.into_reader())),
                            None,
                        );
                    }
                }
            }

            if compression_used {
                let vary = conn
                    .response_headers_mut()
                    .get_str(Vary)
                    .map(|vary| HeaderValues::from(format!("{vary}, Accept-Encoding")))
                    .unwrap_or_else(|| HeaderValues::from("Accept-Encoding"));

                conn.response_headers_mut().extend([
                    (ContentEncoding, HeaderValues::from(algo.as_str())),
                    (Vary, vary),
                ]);
            }

            conn.with_body(body)
        } else {
            conn
        }
    }
}

/// Alias for [`Compression::new`](crate::Compression::new)
pub fn compression() -> Compression {
    Compression::new()
}
