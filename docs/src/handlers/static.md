## Static file serving

Trillium offers two rudimentary approaches to static file serving for now. Neither of these approaches perform any cache-related header checking yet.

### From disk
This handler loads content from disk at request, and does not yet do any in-memory caching.

[rustdocs (main)](https://docs.trillium.rs/trillium_static/index.html)


```rust,noplaypen
{{#include ../../../static/examples/static.rs}}
```

### From memory, at compile time
This handler includes all of the static content in the compiled binary, allowing it to be shipped independently from the assets.

[rustdocs (main)](https://docs.trillium.rs/trillium_static_compiled/index.html)

```rust,noplaypen
{{#include ../../../static-compiled/examples/static-compiled.rs}}
```
