# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Who you're working with

Jacob (`jbr` in code and commits) is the primary maintainer of this workspace and an expert Rust developer. Address him by name rather than as "the user" — this is collaboration, not tool use. Ask directly when something's unclear instead of exploring independently; he's almost always in the loop and it's faster.

## Commands

**Always use `cargo terse` for `check`, `clippy`, and `build`.** It runs the underlying cargo command and prints one line per diagnostic with a stable ID (`E-...`, `W-...`); pull full context for any ID with `cargo terse detail <ID>` (cached — no rebuild). Cargo flags go before `--`, tool flags after. **For tests, prefer `cargo nextest run` over `cargo test`.** This workspace's `.envrc` exports `NEXTEST_PROFILE=terse` (defined in `.config/nextest.toml`), so bare nextest runs already suppress per-test PASS lines and inline failure detail — output is bounded by the number of failures, not the number of tests.

**Never pipe cargo output through `tail` or `head`.** Output is already concise, and `tail` on a build failure cuts off the first error — the one that usually matters. Patterns putatively motivated by context-window efficiency (running the same command twice for head and tail, or guessing line counts) are typically *less* efficient than just reading the whole output. If a specific invocation is genuinely noisy enough to need filtering, call it out and justify it.

```bash
# Check compilation frequently during development
cargo terse check -p trillium-crate-name

# Run focused tests — terse profile is on by default via .envrc
cargo nextest run -p trillium-router

# Drill into a single failure with full panic/assertion detail
cargo nextest run --failure-output=immediate -E 'test(=tests::path::failing_name)'

# Lint (see clippy rule below — always --fix, never bare)
cargo terse clippy --fix --allow-dirty --allow-staged

# Pull full context for a specific diagnostic ID from the cached run
cargo terse detail E-82ee

# Format — just run it. No need for --check locally; Jacob will reformat at checkin if needed.
cargo fmt --all

# Docs
cargo doc --no-deps

# Full workspace test (with runtime feature — slow, let Jacob run this)
cargo nextest run --workspace --features trillium-static/tokio,trillium-testing/tokio
```

**Do not use `--all-features`.** In this workspace features are often toggle states rather than purely additive, and some crates have mutually exclusive feature sets that fail to build together. `--all-features` rarely expands coverage and frequently breaks the build.

**Always run clippy with `--fix`. Never run bare `cargo clippy`.** The correct invocation is `cargo terse clippy --fix --allow-dirty --allow-staged` — this is not a suggestion, it's the default. Clippy can codemod the vast majority of its own lints; running bare clippy (or `clippy --all-targets`) and then hand-editing code to satisfy the output is pure busywork — clippy would have fixed it for you. The `--allow-dirty --allow-staged` flags let `--fix` work on a tree with uncommitted changes, which is almost always what you want. **If you find yourself about to edit a line in response to a clippy warning, stop — you skipped `--fix`.** If clippy ever breaks something by auto-fixing (Jacob has never seen this happen), revert and report it.

## Architecture

Trillium is a modular async web framework workspace (~41 crates). The distinguishing design choice is that **there is no distinction between middleware and endpoints** — everything is a `Handler`.

### Core Crates

- **trillium** — Main framework crate, exports `Handler`, `Conn`, `State`
- **trillium-http** — Low-level HTTP/1.x and HTTP/3 protocol, parsing, headers
- **trillium-quinn** — HTTP/3 support via Quinn (QUIC); add alongside existing TLS config with `.with_quic(...)`
- **trillium-server-common** — Shared server implementation
- **trillium-macros** — Proc macros
- **trillium-testing** — Testing utilities (`assert_ok!`, `assert_not_handled!`, etc.)

### Runtime Adapters (pick one per application)

**trillium-tokio**, **trillium-smol**, **trillium-async-std** — These are mutually exclusive runtime adapters. Most crates are runtime-agnostic; runtime is selected at the application level.

### Handler Trait

```rust
// Central abstraction — no #[async_trait] needed in 1.0
async fn run(&self, conn: Conn) -> Conn
```

Handlers are composed via tuples:
```rust
(logger(), compression(), cookies(), router(), static_files())
```

There is no separate "middleware" concept. Tuple handlers run left-to-right, **stopping at the first handler that halts the conn**. A handler halts by calling `conn.halt()` (or helpers like `conn.ok(...)` which halt implicitly).

The `trillium` crate exports `State<T>` (clones a value into each conn's state), `Init` (closure-based startup initializer for shared server-level state), and `BoxedHandler` (type-erased handler; use instead of `Box<dyn Handler>`). All other handlers live in separate crates.

**Built-in `Handler` impls** (useful in tests and examples):
- `()` — noop, passes conn through unchanged
- `&'static str` / `String` — halts with 200 + string body
- `Option<impl Handler>` — noop if `None`, useful for conditional handlers

### Conn

`Conn` represents both the HTTP request and response as a single object that flows through handlers. It also owns the underlying TCP connection — dropping a `Conn` disconnects the client.

State is stored in a `TypeSet` (one value per type), accessed via trait extensions. Conn supports a fluent builder interface for setting response properties:
```rust
conn.with_status(202)
    .with_response_header("content-type", "text/plain")
    .with_body("hello")
```

Default response is 404 with no body, so returning an unmodified conn is always valid.

**Pattern used by library crates:** Store state using a private newtype, expose access via a `[Something]ConnExt` trait. This avoids type conflicts between crates since `TypeSet` holds one value per type.

### Transport Abstraction

`BoxedTransport` is used in public API, allowing runtime transport selection without making `Conn` generic. TCP, TLS (rustls or native-tls), and other transports all implement the same transport trait.

### Server Configuration

Servers read `HOST` and `PORT` from the environment by default (12-factor style). On Unix, `LISTEN_FD` is also supported (for catflap/systemfd). If `HOST` starts with `.`, `/`, or `~`, it is treated as a Unix domain socket path.

## Conventions

**Method naming:**
- `with_{attribute}` — Takes `self`, sets attribute, returns `self` (enables chaining)
- `set_{attribute}` — Takes `&mut self`, returns `&mut Self` (enables chaining on mutable refs)

**Lint profile** (all lib.rs files include):
```rust
#![forbid(unsafe_code)]
#![deny(clippy::dbg_macro, missing_copy_implementations, missing_debug_implementations, ...)]
#![warn(missing_docs, clippy::pedantic, clippy::nursery, clippy::cargo)]
#![allow(clippy::must_use_candidate, clippy::module_name_repetitions)]
```

Every public item needs documentation (`missing_docs` is `warn`-level).

**Commit convention:** Commits prefixed with `feat`, `fix`, or `deps` trigger releases via `release-plz`. Breaking changes use `!` suffix (e.g., `feat!:`).

## Testing

Tests use `trillium-testing`. The preferred approach in 1.0 is `TestServer` (async, method-chained assertions):

```rust
use trillium_testing::{test, TestResult, TestServer, harness};

#[test(harness)]
async fn my_test() -> TestResult {
    let app = TestServer::new(handler).await;
    app.get("/path").await.assert_ok().assert_body("expected body");
    app.get("/missing").await.assert_status(404);
    Ok(())
}
```

The old macro-style assertions (`assert_ok!`, `assert_not_handled!`, etc.) are deprecated but still work — avoid adding new tests with them.

Prefer focused test runs (`cargo nextest run -p crate`) over workspace-wide runs. Workspace-wide tests are slow and noisy — let Jacob run those and summarize results. Drill into a specific failure with `cargo nextest run --failure-output=immediate -E 'test(=path::to::test)'`.

## Working in this Codebase

**Ask questions rather than exploring independently.** Jacob is actively involved — asking directly is faster and more accurate than autonomous investigation.

**Read whole files** rather than using offsets/limits wherever possible — provides complete context and avoids multiple partial reads.

**Avoid spawning agents without discussing first.** Reading files, understanding code structure, and summarizing functionality are almost always faster done directly with Read and Grep.

**When using agents:** Instruct them to halt and report back if they encounter anything unexpected rather than working around it. Returning with "conditions were not as expected" is a success.

**File length:** If you're editing a file that's already over ~500 lines, always raise whether it's worth splitting into submodules before proceeding.

**Dead code:** Delete unused code rather than commenting it out. Version control preserves history.

**Let tools do their own work.** Don't hand-apply fixes that `cargo terse clippy --fix` would codemod, and don't hand-format what `cargo fmt` would format. This isn't thoroughness — it's wasted turns. If you see lint output, your next action is `--fix`, not an `Edit` tool call. Same for format drift: `cargo fmt --all`, not manual whitespace edits.

**Incremental compilation:** Organize changes into the smallest edits that will compile. Run `cargo terse check -p crate` frequently.

**Design decisions are hypotheses.** When unexpected complexity or awkwardness emerges during implementation — something that wasn't discussed — pause and raise it rather than working around it.

**Update plan/memory files as you go.** If the work is being driven by a memory or plan markdown file, edit it incrementally as decisions land and phases complete — don't batch all the updates for end-of-session. Try to remember this, but don't stress about perfection; a mid-session update beats a missed one.

**Suggest tools proactively.** If a crate, CLI, or dev tool would help (linter, benchmarking harness, tracing tool, etc.), mention it. Jacob probably already has it via brew or is happy to install it — don't silently work around a missing tool.

**Several dependencies are Jacob's own crates.** If one of these doesn't have the API we need, say so rather than working around it — changing the upstream is usually on the table: `swansong`, `fieldwork`, `smartcow`, `type-set`, `test-harness`, `querystrong`, `full-duplex-async-copy`, `routefinder` (the routing engine inside `trillium-router`). "This would be cleaner if `fieldwork` exposed X" is a useful observation, not a blocker.

**Looking up Rust APIs — use `ferritin`.** Prefer ferritin over grepping source for anything public-facing. It reads rustdoc, which means it sees fieldwork-generated accessors and derive-macro output that aren't in the source.

- `ferritin get <path>` works against crates.io by default: `ferritin get serde`, `ferritin get bytes@1::Bytes`, `ferritin get tokio@1.35::sync::mpsc::Sender`. Omit the version for latest; semver requirements like `0.1` resolve to the newest matching release. Pre-releases (`-rc.*`, etc.) must be named explicitly.
- `ferritin search <query> -c <crate>` searches a single crate; `ferritin search --local <query>` searches the whole workspace plus its dependencies. First run on a large workspace is slow (builds+indexes all deps) but cached after.
- `ferritin list --local` lists workspace crates.
- `--local` targets the workspace instead of docs.rs. Use it when a nonstandard feature matters or you specifically want to see how a workspace crate's own docs render; otherwise plain `ferritin get <crate>` against crates.io is usually preferable even for locally-present crates.
- Output looks sparse because ferritin auto-detects Claude Code (via the `CLAUDECODE` env var) and switches to a token-concise renderer — that's intentional, not a rendering bug.
- Under active development. If output is confusing, info density feels off, something crashes, or a query returns unexpectedly empty results, flag it — the goal is for ferritin to be the best way to view rustdocs.

**Shell output:** When non-cargo bash output is genuinely large enough to need exploration, redirect to a file, then use Read/Grep. Avoids re-running expensive commands. (For cargo, use `cargo terse` instead — it already structures the output.)

```bash
some-noisy-command > output.txt 2>&1
# then explore with Read, Grep
```

**Communication:** Keep it concise and informal. Skip "what we've accomplished" summaries; focus on what's next or what needs a decision.
