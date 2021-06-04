/**
# Unwraps an Result::Ok or returns the conn with a 500 status.

```
use trillium_testing::{assert_body, assert_status, methods::*};

let handler = |mut conn: trillium::Conn| async move {
  let mut request_body = conn.request_body().await;
  let request_body_string = trillium::conn_try!(conn, request_body.read_string().await);
  let u8: u8 = trillium::conn_try!(conn, request_body_string.parse());
  conn.ok(format!("received u8 as body: {}", u8))
};

assert_status!(
    post("/").with_request_body("not u8").on(&handler),
    500
);

assert_body!(
    post("/").with_request_body("10").on(&handler),
    "received u8 as body: 10"
);


```


*/
#[macro_export]
macro_rules! conn_try {
    ($conn:expr, $expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => {
                log::error!("{}:{} conn_try error: {}", file!(), line!(), error);
                return $conn.with_status(500).halt();
            }
        }
    };
}

/**
# Unwraps an Option::Some or returns the conn.

This is useful for gracefully exiting a Handler without
returning an error.

```
use trillium_testing::{methods::*, assert_not_handled};
struct MyState(&'static str);
let handler = |conn: trillium::Conn| async move {
  let important_state: &MyState = trillium::conn_unwrap!(conn, conn.state());
  let ok_response = String::from(important_state.0);
  conn.ok(ok_response)
};

assert_not_handled!(get("/").on(&handler)); // we never reached the conn.ok line.
```
*/
#[macro_export]
macro_rules! conn_unwrap {
    ($conn:expr, $option:expr) => {
        match $option {
            Some(value) => value,
            None => return $conn,
        }
    };
}

/**
# A convenience macro for logging the contents of error variants.

This
is useful when there is no further action required to process the
error path, but you still want to record that it transpired
*/
#[macro_export]
macro_rules! log_error {
    ($expr:expr) => {
        if let Err(err) = $expr {
            log::error!("{}:{} {:?}", file!(), line!(), err);
        }
    };

    ($expr:expr, $message:expr) => {
        if let Err(err) = $expr {
            log::error!("{}:{} {} {:?}", file!(), line!(), $message, err);
        }
    };
}
