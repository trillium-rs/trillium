# Testing trillium applications

Trillium provides a testing crate that intends to provide both
"functional/unit testing" and "integration testing" of trillium
applications.

[rustdocs (main)](https://docs.trillium.rs/trillium_testing)

Given a totally-contrived application like this:
```rust,noplaypen
{{#include ../../testing/examples/testing.rs:1:21}}
```

Here's what some simple tests would look like:

```rust,noplaypen
{{#include ../../testing/examples/testing.rs:23:}}
```

