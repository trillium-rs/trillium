## Template engines

There are currently three template engines for trillium. Although they are in no way mutually exclusive, most applications will want at most one of these.

### Askama

Askama is a jinja-based template engine that preprocesses templates at
compile time, resulting in efficient and type-safe templates that are
compiled into the application binary. We recommend this approach as a
default. Here's how it looks:

Given the following file in `(cargo root)/templates/examples/hello.html`,
```django
{{#include ../../../askama/templates/examples/hello.html}}
```

```rust,noplaypen
{{#include ../../../askama/examples/askama.rs}}
```

[rustdocs (main)](https://docs.trillium.rs/trillium_askama/index.html)

### Tera

Tera offers runtime templating. Trillium's tera integration provides an interface very similar to `phoenix` or `rails`, with the notion of `assigns` being set on the conn prior to render.


Given the following file in the same directory as main.rs (examples in this case),
```django
{{#include ../../../tera/examples/hello.html}}
```

```rust,noplaypen
{{#include ../../../tera/examples/tera.rs}}
```

[rustdocs (main)](https://docs.trillium.rs/trillium_tera/index.html)

### Handlebars

Handlebars also offers runtime templating. Given the following file in `examples/templates/hello.hbs`,

```handlebars
{{#include ../../../handlebars/examples/templates/hello.hbs}}
```

```rust,noplaypen
{{#include ../../../handlebars/examples/handlebars.rs}}
```

[rustdocs (main)](https://docs.trillium.rs/trillium_handlebars/index.html)
