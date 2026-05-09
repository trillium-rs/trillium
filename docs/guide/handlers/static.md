## Static file serving

Trillium offers two approaches to serving static files: streaming from disk at request time, or
embedding the contents of a directory in the binary at compile time. Both handle precompressed
content negotiation and emit caching-friendly response headers.

### From disk

Loads file content from disk at request time.

[rustdocs (main)](https://docs.trillium.rs/trillium_static/index.html)

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-static = { path = "../static" }
# trillium-logger = { path = "../logger" }
# file: static/examples/files/
#
pub fn main() {
    use trillium_static::{crate_relative_path, files};
    trillium_smol::run((
        trillium_logger::logger(),
        files(crate_relative_path!("files")).with_index_file("index.html"),
    ))
}
```

#### Precompressed sidecars

If your build pipeline produces `.br`, `.zst`, or `.gz` siblings next to your assets, opt in with
`with_precompressed()` and the handler will serve them when the client's `Accept-Encoding` allows
it, in that priority order, with `Content-Encoding` set and the original asset's MIME type
preserved. `Vary: Accept-Encoding` is set on every response from the handler so caches do not
serve a compressed response to a client that did not ask for one.

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-static = { path = "../static" }
#
pub fn main() {
    use trillium_static::{StaticFileHandler, crate_relative_path};
    trillium_smol::run(
        StaticFileHandler::new(crate_relative_path!("files"))
            .with_index_file("index.html")
            .with_precompressed(),
    )
}
```

For non-default codings or suffixes, register variants individually with
`with_precompressed_variant("encoding", "suffix")`.

#### Range requests

Range support is on by default. Every response advertises `Accept-Ranges: bytes`. A single-range
request like `Range: bytes=0-1023` causes the handler to seek into the file and stream just that
byte range with status `206 Partial Content` and a `Content-Range` header. Out-of-bounds ranges
return `416 Requested Range Not Satisfiable`; multi-range requests fall through to a `200` full
body.

`If-Range` is honored with strong-comparison only per RFC 9110. The handler's metadata-derived
etag is weak and so cannot satisfy `If-Range`, but a `Last-Modified` date in `If-Range` will.

Ranged requests bypass precompressed-sidecar selection — the range applies to the identity
representation, never to a compressed sidecar.

### From memory, at compile time

Includes all of the static content in the compiled binary, allowing it to be shipped independently
from the assets.

[rustdocs (main)](https://docs.trillium.rs/trillium_static_compiled/index.html)

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-static-compiled = { path = "../static-compiled" }
# trillium-logger = { path = "../logger" }
# trillium-caching-headers = { path = "../caching-headers" }
# file: static-compiled/examples/files
#
pub fn main() {
    use trillium_static_compiled::static_compiled;

    trillium_smol::run((
        trillium_logger::Logger::new(),
        trillium_caching_headers::CachingHeaders::new(),
        static_compiled!("./files").with_index_file("index.html"),
    ));
}
```

#### Cache validation headers

Two response headers are set by default on every file:

- **`Last-Modified`**, derived from the file's on-disk mtime as captured by the macro at compile
  time.
- **`ETag`**, computed at compile time as `etag::EntityTag::from_data(source)`. The baked tag is
  byte-identical to what `trillium_caching_headers::Etag` would compute at runtime.

Pair this handler with `trillium_caching_headers::CachingHeaders` (as in the example above) to
get full conditional-GET handling for both — `If-Modified-Since` against the file's mtime and
`If-None-Match` against the precomputed etag, returning `304 Not Modified` when either matches.
Because the etag is already set by the time `CachingHeaders` runs, the etag handler skips
rehashing the body entirely.

To opt out of etag generation per invocation, pass `etag = false`:

```rust,ignore
static_compiled!("./files", etag = false)
```

#### Precompression (opt-in)

Pre-compress bundle contents into Brotli, Zstd, and Gzip variants at build time. Enable one or
more encoders via cargo features (`brotli`, `zstd`, `gzip`, or the `compression` meta-feature),
then opt in per invocation:

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-static-compiled = { path = "../static-compiled", features = ["compression"] }
# file: static-compiled/examples/files
#
pub fn main() {
    use trillium_static_compiled::static_compiled;

    trillium_smol::run(
        static_compiled!("./files", compress).with_index_file("index.html"),
    );
}
```

The bare form `compress` bakes every encoding whose feature is enabled. The list form
`compress = [Brotli, Gzip]` bakes a specified subset and is a compile error if a listed
encoding's feature is not enabled.

The macro runs each encoder at maximum quality, in parallel via rayon at macro expansion time.
Per-file variants are sorted smallest-first and only baked when they save at least 5%; files
under 256 bytes are skipped entirely. At request time the handler picks the smallest variant the
client's `Accept-Encoding` allows, sets `Content-Encoding`, and emits `Vary: Accept-Encoding`
(per-file, only when variants are baked).

This composes with [`trillium-compression`](./compression.md), which passes through any response
that already has `Content-Encoding` set — so an upstream `Compression` handler will leave the
precompiled bytes untouched.

#### Range requests

Range support is on by default. Every response advertises `Accept-Ranges: bytes`. A single-range
request like `Range: bytes=0-1023` slices the in-memory bytes and returns them with status `206
Partial Content` and a `Content-Range` header. Out-of-bounds ranges return `416 Requested Range
Not Satisfiable`; multi-range requests fall through to a `200` full body.

`If-Range` is honored with strong-comparison only per RFC 9110, against either the precomputed
strong etag or the `Last-Modified` date.

Ranged requests bypass `Accept-Encoding` negotiation — the range applies to the identity
representation, never to a precompiled variant.
