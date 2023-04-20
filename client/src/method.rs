macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        method!(
            $fn_name,
            $method,
            concat!(
                // yep, macro-generated doctests
                "Builds a new client conn with the ",
                stringify!($fn_name),
                " http method and the provided url.

```
use trillium_testing::prelude::*;
use trillium_smol::ClientConfig;
let conn = trillium_client::",
                stringify!($fn_name),
                "::<ClientConfig>(\"http://localhost:8080/some/route\");

assert_eq!(conn.method(), Method::",
                stringify!($method),
                ");
assert_eq!(conn.url().to_string(), \"http://localhost:8080/some/route\");
```
"
            )
        );
    };
    ($fn_name:ident, $method:ident, $doc_comment:expr) => {
        #[doc = $doc_comment]
        pub fn $fn_name<C, U>(url: U) -> Self
        where
            C: Connector + Default,
            <U as TryInto<Url>>::Error: Debug,
            U: TryInto<Url>,
        {
            Conn::new(C::default(), Method::$method, url)
        }
    };
}
