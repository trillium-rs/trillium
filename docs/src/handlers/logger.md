## Logger

Currently there's just a DevLogger, but soon there will be more
loggers. Trillium loggers use the `log` crate and emit one line at the
info log level per http request/response pair. We like to use the
`env_logger` crate, but any `log` backend will work equally well.

```rust
{{#include ../../../logger/examples/logger.rs}}
```
