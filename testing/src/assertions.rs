/**
assert that the status code of a conn is as specified.

```
use trillium_testing::prelude::*;
async fn handler(conn: trillium::Conn) -> trillium::Conn {
    conn.with_status(418)
}


assert_status!(get("/").on(&handler), 418);
assert_status!(get("/").on(&handler), StatusCode::ImATeapot);

let conn = get("/").on(&handler);
assert_status!(&conn, 418);
assert_status!(conn, 418);
```


```rust,should_panic
use trillium_testing::prelude::*;
async fn handler(conn: trillium::Conn) -> trillium::Conn {
    conn.ok("handled")
}

assert_status!(get("/").on(&handler), 418);
```
*/

#[macro_export]
macro_rules! assert_status {
    ($conn:expr, $status:expr) => {{
        use std::convert::TryInto;
        let expected_status: $crate::StatusCode =
            $status.try_into().expect("expected a status code");

        match $conn.status() {
            Some(status) => assert_eq!(status, expected_status),
            None => panic!("expected status code, but none was set"),
        }
    }};
}

/**
assert that all of the following are true:
* the status was not set
* the body was not set
* the conn was not halted

```
use trillium_testing::prelude::*;
async fn handler(conn: trillium::Conn) -> trillium::Conn {
    conn
}


assert_not_handled!(get("/").on(&handler));

let conn = get("/").on(&handler);
assert_not_handled!(&conn);
assert_not_handled!(conn);
```


```rust,should_panic
use trillium_testing::prelude::*;
async fn handler(conn: trillium::Conn) -> trillium::Conn {
    conn.ok("handled")
}

assert_not_handled!(get("/").on(&handler));
```
*/

#[macro_export]
macro_rules! assert_not_handled {
    ($conn:expr) => {{
        let conn = $conn;
        assert_eq!(conn.status(), None);
        assert!(conn.inner().response_body().is_none());
        assert!(!conn.is_halted());
    }};
}

/**
assert that the response body is as specified. this assertion requires mutation of the conn.

```
use trillium_testing::prelude::*;
async fn handler(conn: trillium::Conn) -> trillium::Conn {
    conn.ok("it's-a-me, trillium")
}


assert_body!(get("/").on(&handler), "it's-a-me, trillium");

let mut conn = get("/").on(&handler);
assert_body!(&mut conn, "it's-a-me, trillium");

let mut conn = get("/").on(&handler);
assert_body!(conn, "it's-a-me, trillium");
```


```rust,should_panic
use trillium_testing::prelude::*;
assert_body!(get("/").on(&()), "what body?");
```

```rust,should_panic
use trillium_testing::prelude::*;
assert_body!(get("/").on(&"beach body"), "winter body");
```
*/

#[macro_export]
macro_rules! assert_body {
    ($conn:expr, $body:expr) => {{
        let body = $conn.take_body_string().expect("body should exist");
        assert_eq!(body.trim_end(), $body.trim_end());
    }};
}

/**

asserts that the response body matches the specified pattern, using [`str::contains`]
```
use trillium_testing::prelude::*;
let handler = "there's a needle in this haystack";
assert_body_contains!(get("/").on(&handler), "needle");

let mut conn = get("/").on(&handler);
let body = assert_body_contains!(&mut conn, "needle");
assert!(body.contains("haystack"));

```


```rust,should_panic
use trillium_testing::prelude::*;
assert_body_contains!(get("/").on(&()), "what body?");
```

```rust,should_panic
use trillium_testing::prelude::*;
assert_body_contains!(get("/").on(&"just a haystack"), "needle");
```
*/

#[macro_export]
macro_rules! assert_body_contains {
    ($conn:expr, $pattern:expr) => {{
        let body = $conn.take_body_string().expect("body should exist");
        assert!(
            body.contains($pattern),
            "\nexpected \n`{}`\n to contain `{}`\n but it did not",
            &body,
            $pattern
        );
        body
    }};
}

/**
combines several other assertions. this assertion can be used to assert:
* just a status code,
* a status code and a response body, or
* a status code, a response body, and any number of headers

```
use trillium_testing::prelude::*;
async fn handler(conn: Conn) -> Conn {
    conn.with_body("just tea stuff here")
        .with_status(418)
        .with_header(("server", "zojirushi"))
}

assert_response!(get("/").on(&handler), 418);
assert_response!(get("/").on(&handler), StatusCode::ImATeapot);
assert_response!(get("/").on(&handler), 418, "just tea stuff here");
assert_response!(get("/").on(&handler), StatusCode::ImATeapot, "just tea stuff here");

assert_response!(
    get("/").on(&handler),
    StatusCode::ImATeapot,
    "just tea stuff here",
    "server" => "zojirushi",
    "content-length" => "19"
);

```


*/

#[macro_export]
macro_rules! assert_response {
    ($conn:expr, $status:expr, $body:expr) => {{
        let mut conn = $conn;
        $crate::assert_status!(conn, $status);
        $crate::assert_body!(conn, $body);
    }};

    ($conn:expr, $status:expr) => {
        $crate::assert_status!($conn, $status);
    };

    ($conn:expr, $status:expr, $body:expr, $($header_name:literal => $header_value:expr,)+) => {
        assert_response!($conn, $status, $body, $($header_name => $header_value),+);
    };

    ($conn:expr, $status:expr, $body:expr, $($header_name:literal => $header_value:expr),*) => {
        let mut conn = $conn;
        $crate::assert_response!(&mut conn, $status, $body);
        $crate::assert_headers!(&conn, $($header_name => $header_value),*);
    };

}

/**
asserts any number of response headers

```
use trillium_testing::prelude::*;
async fn handler(conn: Conn) -> Conn {
    conn.ok("headers")
        .with_header(("server", "special-custom-server"))
        .with_header(("request-id", "10"))
}

assert_headers!(get("/").on(&handler), "server" => "special-custom-server");
assert_headers!(
    get("/").on(&handler),
    "server" => "special-custom-server",
    "request-id" => "10",
    "content-length" => "7"
);

```
*/
#[macro_export]
macro_rules! assert_headers {
    ($conn:expr, $($header_name:literal => $header_value:expr,)+) => {
        assert_headers!($conn, $($key => $value),+);
    };

    ($conn:expr, $($header_name:literal => $header_value:expr),*) => {
        let conn = $conn;
        let headers = conn.inner().response_headers();
        $(
            assert_eq!(
                headers.get($header_name).map(|h| h.as_str()),
                Some($header_value),
                concat!("for header ", $header_name)
            );
        )*
    };
}

/**
assert_ok is like [`assert_response!`] except it always asserts a status of 200 Ok.

it can be used to assert:
* just that the response was successful,
* that the response was successful and a response body, or
* that the response was successful, a response body, and any number of headers

```
use trillium_testing::prelude::*;
async fn handler(conn: Conn) -> Conn {
    conn.ok("body")
        .with_header(("server", "special-custom-server"))
        .with_header(("request-id", "10"))
}

assert_ok!(get("/").on(&handler));
assert_ok!(get("/").on(&handler), "body");
assert_ok!(get("/").on(&handler), "body");
assert_ok!(get("/").on(&handler), "body", "server" => "special-custom-server");

assert_ok!(
    get("/").on(&handler),
    "body",
    "server" => "special-custom-server",
    "request-id" => "10",
    "content-length" => "4"
);

```


*/

#[macro_export]
macro_rules! assert_ok {
    ($conn:expr) => {
        $crate::assert_response!($conn, 200);
    };

    ($conn:expr, $body:expr) => {
        $crate::assert_response!($conn, 200, $body);
    };


    ($conn:expr, $body:expr, $($header_name:literal => $header_value:expr,)+) => {
        assert_ok!($conn, $body, $($header_name => $header_value),+);
    };

    ($conn:expr, $body:expr, $($header_name:literal => $header_value:expr),*) => {
        $crate::assert_response!($conn, 200, $body, $($header_name => $header_value),*);
    };
}
