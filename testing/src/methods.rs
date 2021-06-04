macro_rules! test_conn_method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(path: impl Into<String>) -> $crate::TestConn {
            $crate::TestConn::build($crate::Method::$method, path, ())
        }
    };
}

test_conn_method!(get, Get);
test_conn_method!(post, Post);
test_conn_method!(put, Put);
test_conn_method!(delete, Delete);
test_conn_method!(patch, Patch);
