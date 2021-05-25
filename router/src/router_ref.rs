use crate::Router;
use trillium::{http_types::Method, Handler};

macro_rules! method_ref {
    ($fn_name:ident, $method:ident) => {
        method_ref!(
            $fn_name,
            $method,
            concat!(
                // yep, macro-generated doctests
                "Registers a handler for the ",
                stringify!($fn_name),
                " http method.

```
# use trillium::Conn;
# use trillium_router::Router;
let router = Router::build(|mut router| {
    router.",
                stringify!($fn_name),
                "(\"/some/route\", |conn: Conn| async move {
        conn.ok(\"success\")
    });
});

use trillium_testing::{TestHandler, assert_ok};
let test_handler = TestHandler::new(router);
assert_ok!(test_handler.",
                stringify!($fn_name),
                "(\"/some/route\"), \"success\");
assert!(test_handler.",
                stringify!($fn_name),
                "(\"/other/route\").status().is_none());
```
"
            )
        );
    };

    ($fn_name:ident, $method:ident, $doc_comment:expr) => {
        #[doc = $doc_comment]
        pub fn $fn_name(&mut self, path: &'static str, handler: impl Handler) {
            self.0.add(path, Method::$method, handler);
        }
    };
}

/**
# A `&mut Router` for use with `Router::build`

A wrapper around a `&mut Router` that supports imperative route
registration. See [`Router::build`] for further documentation.
*/
#[derive(Debug)]
pub struct RouterRef<'r>(&'r mut Router);
impl<'r> RouterRef<'r> {
    method_ref!(get, Get);
    method_ref!(post, Post);
    method_ref!(put, Put);
    method_ref!(delete, Delete);
    method_ref!(patch, Patch);

    /**
    Appends the handler to all (get, post, put, delete, and patch) methods.
    ```
    # use trillium::Conn;
    # use trillium_router::Router;
    let router = Router::build(|mut router|
        router.any("/any", |conn: Conn| async move {
            let response = format!("you made a {} request to /any", conn.method());
            conn.ok(response)
        });
    });

    use trillium_testing::{TestHandler, assert_ok};
    let test_handler = TestHandler::new(router);
    assert_ok!(test_handler.get("/any"), "you made a GET request to /any");
    assert_ok!(test_handler.post("/any"), "you made a POST request to /any");
    assert_ok!(test_handler.delete("/any"), "you made a DELETE request to /any");
    assert_ok!(test_handler.patch("/any"), "you made a PATCH request to /any");
    assert_ok!(test_handler.put("/any"), "you made a PUT request to /any");

    assert!(test_handler.get("/").status().is_none());
    ```

    */
    pub fn any(&mut self, path: &'static str, handler: impl Handler) {
        self.0.add_any(path, handler)
    }

    pub(crate) fn new(router: &'r mut Router) -> Self {
        Self(router)
    }
}
