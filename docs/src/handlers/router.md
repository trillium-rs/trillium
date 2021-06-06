# Router

[rustdocs (main)](https://docs.trillium.rs/trillium_router/index.html)

The trillium router is based on
[routefinder](https://github.com/jbr/routefinder). This router
supports two types of patterns: Untyped params and a single
wildcard. Named params are captured in a map-like interface. Any
handler can be mounted inside of a Router (including other Routers),
allowing entire applications to be mounted on a path, and allowing for
tuple handlers to be run on a given route. Any handler mounted inside
of a route that includes a `*` will have the url rewritten to the
contents of that star.

Alternative routers that are not based on routefinder are a prime
opportunity for innovation and exploration.

Here's a simple example of an application that responds to a request
like http://localhost:8000/greet/earth with "hello earth" and
http://localhost:8000/greet/mars with "hello mars" and responds to
http://localhost:8000 with "hello everyone"

```rust,noplaypen
{{#include ../../../router/examples/router.rs}}
```


## Nesting

Trillium also supports nesting routers, making it possible to express
complex sub-applications, vaguely along the lines of a [rails
engine](https://guides.rubyonrails.org/engines.html). When there are
additional types of routers, it will be possible for an application
built with one type of router to be published as a crate and nested
inside of another router as long as they depend on a compatible
version of the `trillium` crate.

```rust,noplaypen
{{#include ../../../router/examples/nested-router.rs}}
```
