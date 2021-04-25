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

```rust
{{#include ../../../askama/examples/askama.rs}}
```

### Tera

Tera offers runtime templating. Trillium's tera integration provides an interface very similar to `phoenix` or `rails`, with the notion of `assigns` being set on the conn prior to render.


Given the following file in the same directory as main.rs (examples in this case),
```django
{{#include ../../../tera/examples/hello.html}}
```

```rust
{{#include ../../../tera/examples/tera.rs}}
```

### Handlebars

Handlebars also offers runtime templating. Given the following file in `examples/templates/hello.hbs`,

```handlebars
{{#include ../../../handlebars/examples/templates/hello.hbs}}
```

```rust
{{#include ../../../handlebars/examples/handlebars.rs}}
```


