# Patterns for library authors

## State

Let's take a look at an implementation of a library that incrementally counts the number of conns that pass through it and attaches the number to each conn. It would be unsafe to store a u64 directly in the state set, because other libraries might be doing so, so we wrap it with a private newtype called ConnNumber. Since this isn't accessible outside of our library, we can be sure that our handler is the only place that sets it.  We provide a ConnExt trait in order to provide access to this data.

```rust,noplaypen
{{#include ../../trillium/examples/state.rs:1:38}}
```

And usage of the library looks like this:


```rust,noplaypen
{{#include ../../trillium/examples/state.rs:40:}}
```
