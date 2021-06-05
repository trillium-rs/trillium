## Logger

[rustdocs (main)](https://docs.trillium.rs/trillium_logger/)

Currently the logger is incredibly simple. The Trillium logger uses
the `log` crate and emit one line at the info log level per http
request/response pair. The trillium docs use the `env_logger` crate,
but any `log` backend will work equally well. Trillium crates will
never emit at an `info` level other than in a logger crate other than
when starting up or shutting down.

```rust
{{#include ../../../logger/examples/logger.rs}}
```


