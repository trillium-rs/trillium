# Logger

[rustdocs](https://docs.trillium.rs/trillium_logger)

The `trillium-logger` crate logs each HTTP request after the response is sent. It runs as a `before_send` hook so it can include the final response status and body size.

## Basic usage

```rust,noplaypen
use trillium_logger::Logger;

fn main() {
    trillium_smol::run((
        Logger::new(),
        |conn: trillium::Conn| async move { conn.ok("hello") },
    ));
}
```

`Logger::new()` uses the `dev_formatter` by default, which produces compact, colored output suitable for development:

```
GET /path 200 OK 12b 1.2ms
```

A convenience function `logger()` is also provided as a shorthand for `Logger::new()`.

## Built-in formatters

The `formatters` module provides building blocks for common log formats:

**`dev_formatter`** — compact, colorized, human-readable. Used by default.

**`apache_common`** — the [Apache Common Log Format](https://httpd.apache.org/docs/current/logs.html#common):
```
127.0.0.1 - frank [timestamp] "GET /index.html HTTP/1.1" 200 2326
```

**`apache_combined`** — extends Apache Common with `Referer` and `User-Agent` fields.

Both Apache formats require two arguments: a request identifier placeholder and a user identifier placeholder. These can be string literals or custom formatter functions:

```rust,noplaypen
use trillium_logger::{Logger, apache_combined};

Logger::new().with_formatter(apache_combined("-", "-"))
```

## Custom formatters

Formatters are composable. Tuple formatters concatenate their parts, and any `fn(&Conn, bool) -> impl Display` works as a formatter component. The `bool` argument indicates whether color output is enabled.

```rust,noplaypen
use trillium::{Conn, State};
use trillium_logger::{Logger, apache_combined, formatters};

#[derive(Clone, Copy)]
struct User(&'static str);

fn user_id(conn: &Conn, _color: bool) -> &'static str {
    conn.state::<User>().map(|u| u.0).unwrap_or("-")
}

// Include a custom user field in the log line
Logger::new().with_formatter(apache_combined("-", user_id))
```

Individual formatter components from the `formatters` module can be assembled into a tuple for a fully custom format:

```rust,noplaypen
use trillium_logger::{Logger, formatters};

Logger::new().with_formatter((
    "-> ",
    formatters::method,
    " ",
    formatters::url,
    " ",
    formatters::status,
    " ",
    formatters::body_len_human,
))
```

Available components include: `method`, `url`, `status`, `ip`, `timestamp`, `body_len_human`, `bytes`, `request_header(name)`, `response_header(name)`.

## Output target

By default, log lines go to stdout. To route them to the `log` crate instead (for integration with `env_logger`, `tracing`, etc.):

```rust,noplaypen
use trillium_logger::{Logger, Target};

Logger::new().with_target(Target::Logger(log::Level::Info))
```

You can also supply any `Fn(String) + Send + Sync + 'static` as a custom target.

## Color

Color output is auto-detected based on whether stdout is a TTY. Override with:

```rust,noplaypen
use trillium_logger::{ColorMode, Logger};

Logger::new().with_color_mode(ColorMode::Off) // always off
```

## Full example

```rust,noplaypen
{{#include ../../../logger/examples/logger.rs}}
```
