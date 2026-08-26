# Cache

[rustdocs](https://docs.rs/trillium-cache)

The `trillium-cache` crate is an HTTP cache implementing [RFC 9111](https://www.rfc-editor.org/rfc/rfc9111) semantics. Its `Cache` handler sits before another handler and caches that handler's responses, serving them without running it again while they're fresh.

This is a different job than [`trillium-caching-headers`](./utilities.md#caching-headers), which computes `ETag`s and answers conditional requests but stores nothing. `trillium-cache` stores responses and decides when they can be served without running the downstream handler at all.

## Setup

Place `Cache` before the handler whose responses you want to cache:

```rust
# [dependencies]
# trillium = "1"
# trillium-smol = "0.7"
# trillium-logger = "0.5"
# trillium-cache = "0.2"
#
use trillium_cache::{Cache, InMemoryStorage};
use trillium_logger::Logger;

fn main() {
    trillium_smol::run((
        Logger::new(),
        Cache::new(InMemoryStorage::new()).shared(),
        "an expensive response",
    ));
}
```

Whether a response is stored, for how long it counts as fresh, and when it must be revalidated all follow the response's own caching headers (`cache-control`, `expires`, `vary`, validators) per RFC 9111 — the handler brings the mechanism, the application's headers set the policy.

## Private vs shared

The default is private-cache (single-user, browser-style) semantics. A server-side cache in front of an application serves every user from the same store, which is what `.shared()` declares: it makes the cache obey the directives addressed to shared caches, like `s-maxage` and `private`. If the cached handler ever varies responses per user, omitting `.shared()` will serve one user's response to another.

## Storage backends

Storage is a trait, `CacheStorage`, with three implementations provided:

- `InMemoryStorage` — built in, with a byte cap and time-to-idle/time-to-live eviction knobs.
- `FileSystemStorage` — on-disk persistence, behind the `fs` feature.
- `TieredStorage` — composes a fast hot tier over a durable cold tier (the headline pairing is in-memory over filesystem). It is itself a `CacheStorage`, so it drops in wherever a single backend would.

## Streaming and size limits

Cacheable responses stream through the cache: bytes are forwarded to the client and written to storage concurrently, so caching doesn't add a buffering hop. Bodies over a cap (16 MiB by default, settable with `with_max_cacheable_size`) pass through without being stored; if the cap is exceeded mid-stream, the cache write is aborted and the rest of the body passes through unmodified.

## Coverage

The server handler implements storability, freshness, conditional revalidation, `Vary`, invalidation on unsafe methods, and `stale-if-error` recovery ([RFC 5861](https://www.rfc-editor.org/rfc/rfc5861)): when the downstream handler produces a 5xx and a stored entry is eligible, the stored entry is served instead. `stale-while-revalidate` is honored as synchronous revalidation.
