# Pre-Release Review Notes (trillium + trillium-http)

Reviewed by Claude Sonnet 4.6 on 2026-04-07. All files in `trillium/src/` and `http/src/` were read in full, along with the blog post and changelogs.

---

## Fixes Made

### 1. `trillium/src/lib.rs` — stale "pre 1.0" statement
The crate-level doc said "trillium is still pre 1.0 and should be expected to evolve over time." Updated to reflect stable 1.0 release.

### 2. `trillium/src/handler.rs` — grammatical error in `before_send` doc
"the conn **has was** halted" → "was halted"

### 3. `trillium/src/request_body.rs` — truncated doc sentence
`read_string` doc comment ended mid-sentence: "use [`RequestBody::with_max_len`] or" — completed with "or [`RequestBody::set_max_len`]"

### 4. `docs/blog/2026-03-30-trillium-1-0.md` — date frontmatter mismatch
File is named `2026-03-30-trillium-1-0.md` but frontmatter had `date: 2026-03-15`. Updated to `2026-03-30`.

### 5. `docs/blog/2026-03-30-trillium-1-0.md` — grammar ("listener are")
"the tcp listener are bound to" → "the tcp listeners are bound to"

### 6. `docs/blog/2026-03-30-trillium-1-0.md` — paragraph runs into `##` heading
The `RequestBody` section heading and the following paragraph text were on the same line with no blank line. Fixed formatting.

### 7. `http/src/received_body.rs` — Debug impl says "RequestBody" not "ReceivedBody"
`f.debug_struct("RequestBody")` → `f.debug_struct("ReceivedBody")`

### 8. `http/src/body.rs` — broken rustdoc link
`"constructed with \`[Body::new_streaming\`]"` had an extra `[` before the backtick, making the link malformed. Fixed.

### 9. `http/src/received_body.rs` — stale "next semver-minor" note
"In the next semver-minor release, this value will decrease substantially." — This has never happened; at 1.0 the default is still 500mb. The phrasing implies an imminent change that never came. Updated to remove the false promise.

### 10. `http/src/received_body.rs` — "temporary" limitation for large chunks / small buffers
"This limitation is temporary" — still present at 1.0. Updated to remove "temporary" since this hasn't been fixed.

### 11. `trillium/src/transport.rs` — stale version numbers and "temporary situation" note
Doc references "futures 0.3.15" and "tokio 1.6.0" (both very old) and says "Hopefully this is a temporary situation" about the trait divergence. Updated to remove the specific old version numbers and the "temporary" qualifier; the futures-lite/tokio trait split is a permanent design reality.

---

## Design Questions Requiring Decision

### A. `trillium/src/shared_state.rs` — orphaned file (not compiled)
`shared_state.rs` defines `SharedState<T>` (a Handler that inserts a value into shared server state during `init`) and the `shared_state()` constructor. However, `trillium/src/lib.rs` has no `mod shared_state;` statement, so **this file is never compiled**.

Two choices:
1. **Export it** — add `mod shared_state; pub use shared_state::{SharedState, shared_state};` to `lib.rs`. It's a useful convenience for synchronous shared-state setup (simpler alternative to `Init::new` for cases where no async is needed).
2. **Delete it** — if `Init::new` is the only intended API for shared state setup, this is dead code and should be removed per project conventions.

The doc comment reads "This handler populates a type into the immutable server-shared state type-set. Note that unlike `State`, this handler does not require `Clone`..." — suggests it was meant to be public.

### B. `http/src/transport/boxed_transport.rs` — orphaned file with placeholder doc
This file is at `http/src/transport/boxed_transport.rs` but there is no `mod transport` anywhere in `http/src/lib.rs`, so it is never compiled. Its doc comment is `/// tmp` (clearly a placeholder). The http changelog says "BoxedTransport remains as a type alias" but this is currently false.

Two choices:
1. **Wire it up and export it** — add transport module and re-export `BoxedTransport` from `trillium_http`. The architecture docs and testing crate reference this type by name.
2. **Delete it** — if `Box<dyn Transport>` is the intended idiom, the type alias is unnecessary.

### C. http CHANGELOG — incorrect claim about BoxedTransport
The http changelog says: `"BoxedTransport remains as a type alias"` — currently false (it's in a dead file). This should be corrected once decision B above is made.

---

## Minor Issues Noted (lower priority)

- `trillium/src/conn.rs` `response_len` doc example: the example sets a body and checks `body.len()` but the parent method `response_len` is about the conn's length — the example works but doesn't actually call `conn.response_len()`, making it slightly misleading.
- `docs/guide/architecture.md` references `BoxedTransport` — will need updating once decision B is resolved.
- `testing/src/with_server.rs` references `BoxedTransport` — same.
- `http/src/conn.rs` response_body field doc examples use the deprecated `HttpTest` API (deprecated in favour of `TestServer`) — not broken, but worth noting for future cleanup.
- `trillium/src/lib.rs` exports `SharedState`/`shared_state` as nothing (orphan), meaning the changelog entry about `conn.shared_state::<T>()` is fine but if users want a simple non-async shared-state handler they have none.
