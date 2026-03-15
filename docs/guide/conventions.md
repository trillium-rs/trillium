# About this document

Here are some conventions used throughout these docs.

- `use` declarations are listed only on the first usage of a given type, to keep code samples concise.
- Inline code like `|conn: Conn| async move { conn }` uses short, unqualified paths.
- Footnotes look like this[^1].
- Informational asides:
  > ℹ️ Fun fact: facts are fun
- Advanced asides for library authors and edge cases:
  > 🧑‍🎓 The `Handler` trait provides several lifecycle hooks beyond `run`
- Comparisons with Elixir's Plug and Phoenix (a primary architectural inspiration):
  > 🔌 Halting a plug looks like `conn |> halt` (Elixir); the Trillium equivalent is `conn.halt()`

[^1]: Footnotes can always be skipped without losing the thread

## Who is this document for?

This document expects some familiarity with async Rust. If you're new to async Rust, the [Rust Book](https://doc.rust-lang.org/book/) and the [Async Book](https://rust-lang.github.io/async-book/) are good starting points.

It also assumes general web development familiarity — HTTP, request/response cycles, and what middleware means in a web framework context.
