# Welcome to the `trillium-macros` crate!

This crate derive macros for `Handler`, `AsyncRead`, and `AsyncWrite`.

## `derive(Handler)`

This crate currently offers a derive macro for Handler that can be
used to delegate Handler behavior to a contained Handler
type. Currently it only works for structs, but will eventually support
enums as well. Note that it will only delegate to a single inner Handler type.

In the case of a newtype struct or named struct with only a single
field, `#[derive(Handler)]` is all that's required. If there is more
than one field in the struct, annotate exactly one of them with
`#[handler]`.

As of v0.0.2, deriving Handler makes an effort at adding Handler
bounds to generics contained within the `#[handler]` type. It may be
overzealous in adding those bounds, in which case you'll need to
implement Handler yourself.


```rust
// for these examples, we are using a `&'static str` as the handler type.
use trillium_macros::Handler;
# fn assert_handler(_h: impl trillium::Handler) {}

#[derive(Handler)]
struct NewType(&'static str);
assert_handler(NewType("yep"));

#[derive(Handler)]
struct TwoTypes(usize, #[handler] &'static str);
assert_handler(TwoTypes(2, "yep"));

#[derive(Handler)]
struct NamedSingleField {
    this_is_the_handler: &'static str,
}
assert_handler(NamedSingleField { this_is_the_handler: "yep" });


#[derive(Handler)]
struct NamedMultiField {
    not_handler: usize,
    #[handler]
    inner_handler: &'static str,
    also_not_handler: usize,
}

assert_handler(NamedMultiField {
    not_handler: 1,
    inner_handler: "yep",
    also_not_handler: 3,
});

#[derive(Handler)]
struct Generic<G>(G);
assert_handler(Generic("hi"));
assert_handler(Generic(trillium::Status::Ok));


#[derive(Handler)]
struct ContainsHandler<A, B> {
    the_handler: (A, B)
}
assert_handler(ContainsHandler {
    the_handler: ("hi", trillium::Status::Ok)
});

```

### Overriding a single trait function

Annotate the handler with a
[`trillium::Handler`](https://docs.rs/trillium/latest/trillium/trait.Handler.html)
function name `#[handler(overrride = FN_NAME)]` where `FN_NAME` is one of
`run`, `before_send`, `name`, `has_upgrade`, or `upgrade`, and
implement the same signature on Self. The rest of the Handler
interface will be delegated to the inner Handler, but your custom
implementation for the specified function will be called instead.

```rust
use trillium_macros::Handler;
# fn assert_handler(_h: impl trillium::Handler) {}

#[derive(Handler)]
struct CustomName {
    #[handler(except = name)]
    inner: &'static str
}

impl CustomName { // note that this is not a trait impl
    fn name(&self) -> std::borrow::Cow<'static, str> {
        format!("custom name ({})", &self.inner).into()
    }
}

let handler = CustomName { inner: "handler" };
assert_eq!(trillium::Handler::name(&handler), "custom name (handler)");
assert_handler(handler);
```

### Overriding several trait functions

Annotate the handler with any number of
[`trillium::Handler`](https://docs.rs/trillium/latest/trillium/trait.Handler.html)
function names `#[handler(except = [run, before_send, name, has_upgrade,
upgrade])]` and implement the trillium Handler signature of that name
on Self.

```rust
use trillium_macros::Handler;
use trillium::Handler;
# fn assert_handler(_h: impl trillium::Handler) {}

#[derive(Handler)]
struct CustomName {
    #[handler(except = [run, before_send])]
    inner: &'static str
}

impl CustomName { // note that this is not a trait impl
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        // this is an uninspired example but we might do something
        // before or after running the inner handler here
        self.inner.run(conn).await
    }

    async fn before_send(&self, conn: trillium::Conn) -> trillium::Conn {
        // this is an uninspired example but we might do something
        // before or after running the inner handler here
        self.inner.before_send(conn).await
    }
}

let handler = CustomName { inner: "handler" };
assert_handler(handler);
```
