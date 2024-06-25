//! [`TestConn`](crate::TestConn) builders for http methods

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        method!(
            $fn_name,
            $method,
            concat!(
                // yep, macro-generated doctests
                "Builds a new [`TestConn`](crate::TestConn) with the ",
                stringify!($fn_name),
                " http method and the provided path.

```
use trillium_testing::prelude::*;

let conn = ",
                stringify!($fn_name),
                "(\"/some/route\").on(&());

assert_eq!(conn.method(), Method::",
                stringify!($method),
                ");
assert_eq!(conn.path(), \"/some/route\");
```
"
            )
        );
    };

    ($fn_name:ident, $method:ident, $doc_comment:expr) => {
        #[doc = $doc_comment]
        pub fn $fn_name(path: impl Into<String>) -> $crate::TestConn {
            $crate::TestConn::build($crate::prelude::Method::$method, path, ())
        }
    };
}

method!(get, Get);
method!(post, Post);
method!(put, Put);
method!(delete, Delete);
method!(patch, Patch);
