# About this document

Here are some conventions in this document.

* `use` declarations will only be listed once on the first usage of a given type in order to keep code samples concise
* In-line code looks like this: `|conn: Conn| async move { conn }` and will generally not involve fully qualified paths
* Footnotes are represented like this[^1]
* Informational asides look like this:
  > ℹ️ Fun fact: This is neither fun, nor a fact
* Advanced asides look like this
  > 🧑‍🎓 The handler trait provides several other lifecycle hooks for library authors
* Comparisons with Tide
  > 🌊 Tide endpoints look like `|_req: Request<_>| async { Response::new(200) }` whereas Trillium handlers look like `|conn: Conn| async move { conn.status(200) }`
* Comparisons with Plug:
  > 🔌 Halting a plug looks like `conn |> halt` (elixir), and the equivalent in trillium is returning `conn.halt()`


[^1]: Footnotes can always be skipped
