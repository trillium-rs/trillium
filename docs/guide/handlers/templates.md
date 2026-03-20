## Template engines

There are currently three template engines for trillium. Although they are in no way mutually exclusive, most applications will want at most one of these.

### Askama

Askama is a jinja-based template engine that preprocesses templates at
compile time, resulting in efficient and type-safe templates that are
compiled into the application binary. Here's how it looks:

Given the following file in `(cargo root)/templates/examples/hello.html`,
```html
Hello, {{ name }}!
```

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-askama = { path = "../askama" }
# askama = "0.15"
# file: askama/templates
#
use trillium::Conn;
use trillium_askama::AskamaConnExt;
use askama::Template;

#[derive(Template)]
#[template(path = "examples/hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

fn main() {
    trillium_smol::run(|conn: Conn| async move { conn.render(HelloTemplate { name: "world" }) });
}
```

[rustdocs (main)](https://docs.trillium.rs/trillium_askama/index.html)

### Ructe

Ructe is a compile-time typed template system similar to askama, but using a build script instead of macros.

* crate: https://crates.io/crates/trillium-ructe
* repository: https://github.com/prabirshrestha/trillium-ructe
* docs: https://docs.rs/trillium-ructe/latest/trillium_ructe/

### Tera

Tera offers runtime templating. Trillium's tera integration provides an interface very similar to `phoenix` or `rails`, with the notion of `assigns` being set on the conn prior to render.

Given the following file in the same directory as main.rs (examples in this case),
```html
Hello, {{ name }}!
```

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-tera = { path = "../tera" }
# file: tera/examples/hello.html
#
use trillium::Conn;
use trillium_tera::{TeraConnExt, TeraHandler};

fn main() {
    trillium_smol::run((TeraHandler::new("**/*.html"), |conn: Conn| async move {
        conn.assign("name", "hi").render("hello.html")
    }));
}
```

[rustdocs (main)](https://docs.trillium.rs/trillium_tera/index.html)

### Handlebars

Handlebars also offers runtime templating. Given the following file in `examples/templates/hello.hbs`,

```handlebars
hello {{name}}!
```

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-handlebars = { path = "../handlebars" }
# env_logger = "*"
# file: handlebars/examples/templates
#
use trillium::Conn;
use trillium_handlebars::{HandlebarsConnExt, HandlebarsHandler};

fn main() {
    env_logger::init();
    trillium_smol::run((
        HandlebarsHandler::new("templates/*.hbs"),
        |conn: Conn| async move {
            conn.assign("name", "world")
                .render("templates/hello.hbs")
        },
    ));
}
```

[rustdocs (main)](https://docs.trillium.rs/trillium_handlebars/index.html)

