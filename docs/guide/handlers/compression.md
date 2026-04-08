# Compression

[rustdocs](https://docs.trillium.rs/trillium_compression)

The `trillium-compression` crate compresses response bodies transparently. It supports zstd, brotli, and gzip, selecting the algorithm based on the client's `Accept-Encoding` header. Downstream handlers require no modification — they write their response bodies as usual, and compression is applied automatically.

## Basic usage

Place `Compression` early in your handler chain so it wraps all downstream responses:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-compression = { path = "../compression" }
#
use trillium_compression::Compression;

fn main() {
    trillium_smol::run((
        Compression::new(),
        |conn: trillium::Conn| async move { conn.ok("hello world") },
    ));
}
```

A convenience function `compression()` is provided as a shorthand for `Compression::new()`.

## Algorithm selection

When a client sends an `Accept-Encoding` header, the handler picks the best supported algorithm from the client's preferences. The default preference order is **zstd > brotli > gzip**. If the client does not send `Accept-Encoding`, or requests only unsupported encodings, the response body is sent uncompressed.

## Restricting algorithms

If you need to limit which algorithms are offered — for example, to ensure compatibility with clients that don't support newer codecs — use `with_algorithms`:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-compression = { path = "../compression" }
#
use trillium_compression::{Compression, CompressionAlgorithm};

fn main() {
    trillium_smol::run((
        Compression::new().with_algorithms(&[CompressionAlgorithm::Gzip]),
        |conn: trillium::Conn| async move { conn.ok("hello world") },
    ));
}
```

Available variants are `CompressionAlgorithm::Zstd`, `CompressionAlgorithm::Brotli`, and `CompressionAlgorithm::Gzip`.
