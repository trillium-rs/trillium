# Frontend

[rustdocs](https://docs.rs/trillium-frontend)

The `trillium-frontend` crate serves a compiled JS/TS frontend project — Vite, webpack, or Next.js — from a trillium application, so a single binary carries both the api and the app that talks to it. The same handler adapts to context:

| Mode | When | What happens |
|------|------|--------------|
| **Build** | source available, `dev-proxy` feature off | runs the frontend build at compile time and embeds the dist assets in the binary (via `trillium-static-compiled`) |
| **Dev-proxy** | source available, `dev-proxy` feature on | spawns the framework's dev server on a free port and proxies requests to it, including the WebSocket upgrades HMR uses |
| **Prebuilt** | no `package.json` at the project path | embeds already-built dist assets without attempting a build — this is what `cargo install` of a published crate sees |

## Usage

```rust,ignore
use trillium_client::Client;
use trillium_frontend::frontend;
use trillium_smol::ClientConfig;

fn main() {
    trillium_smol::run((
        frontend!("./client")
            .with_client(Client::new(ClientConfig::default()))
            .with_index_file("index.html"),
    ));
}
```

The path is relative to the calling crate's `Cargo.toml`. The same code works in all three modes; `.with_client()` supplies the client the dev proxy uses and is ignored otherwise. Package manager and framework are auto-detected from lock and config files, and can be overridden:

```rust,ignore
frontend!(
    path = "./client",
    build = "bun run build",
    dist = "dist",
)
```

## Development with live reload

The `dev-proxy` feature is what switches on the proxy mode. Forward it through a feature of your own crate so day-to-day development is one flag:

```toml
[features]
dev-proxy = ["trillium-frontend/dev-proxy"]
```

```sh
cargo run --features dev-proxy   # development: live-reloading dev server behind a proxy
cargo build --release            # production: build and embed assets at compile time
```

## SPA fallback

`.with_index_file("index.html")` opts into serving the index for paths no asset matched, which is what makes client-side routing survive a page reload. Only `GET` and `HEAD` requests are eligible — a `POST` to an arbitrary path is not a client-side route, and answering it with the index would misrepresent it as one. `.with_index_predicate(...)` narrows the fallback further to the paths the app actually routes, so that everything else 404s.

## Rebuilds when only frontend sources change

In build mode the frontend build runs inside a proc macro, and on stable Rust proc macros can't tell cargo about file dependencies — so editing *only* a JS file may not trigger a rebuild. On nightly this is handled automatically. If you build on stable, the crate provides an optional build-script shim that registers the frontend sources as compile inputs; dev-proxy mode needs none of this. See the [rustdocs](https://docs.rs/trillium-frontend) for setup.

## Publishing for `cargo install`

To let users `cargo install` your application without JS tooling on their machine, build the frontend, include the dist directory in the published crate, and exclude `package.json` — its absence is what selects prebuilt mode:

```toml
[package]
include = [
    "src/",
    "client/dist/",
]
```
