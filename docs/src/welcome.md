# Welcome

Hi! Welcome to the documentation for Trillium, a modular toolkit for
building async rust web applications.

Trillium runs on stable rust, is fully async, and can run on tokio,
async-std, or smol. Using Trillium starts with code as simple as this:

```rust,noplaypen
fn main() {
    trillium_smol::run(|conn: trillium::Conn| async move {
        conn.ok("hello from trillium!")
    });
}
```

Trillium is also built to scale up to complex applications
with a full middleware stack comparable to Rails or
Phoenix. Currently, opt-in features include a router, cookies,
sessions, websockets, serving static files from disk or memory, a
reverse proxy, and integrations for three template engine
options. Trillium is just getting started, though, and there's a lot
more to build.

Perhaps most importantly, Trillium intends to be a production-quality
open source http framework for async rust, with support options available
for commercial users.

Trillium's code is at
[github](https://github.com/trillium-rs/trillium) and rustdocs are
available at [docs.trillium.rs](https://docs.trillium.rs).
