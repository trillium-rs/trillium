/**
# Builds a new Sequence.

See [`trillium::Sequence`](crate::Sequence) for more information.

```
let macro_sequence = trillium::sequence![trillium_logger::DevLogger, "hello"];
let literal_sequence = trillium::Sequence::new().then(trillium_logger::DevLogger).then("hello");
assert_eq!(format!("{:?}", macro_sequence), format!("{:?}", literal_sequence));
```
*/

#[macro_export]
macro_rules! sequence {
    ($($x:expr),+ $(,)?) => { $crate::Sequence::new()$(.then($x))+ }
}

/**
# Unwrap an Ok Result or returns the conn with a 500 status.

```
use trillium_testing::{assert_body, assert_status, TestConn, TestHandler};

let handler = TestHandler::new(|mut conn: trillium::Conn| async move {
  let mut request_body = conn.request_body().await;
  let request_body_string = trillium::conn_try!(conn, request_body.read_string().await);
  let u8: u8 = trillium::conn_try!(conn, request_body_string.parse());
  conn.ok(format!("received u8 as body: {}", u8))
});

assert_status!(
    TestConn::build("POST", "/", "not u8").run(&handler),
    500
);

assert_body!(
    TestConn::build("POST", "/", "10").run(&handler),
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
# Unwraps an option or returns the conn from the current
scope

This is useful for gracefully exiting a Handler without
returning an error.

```
use trillium_testing::{TestHandler, assert_status};
struct MyState(&'static str);
let handler = TestHandler::new(|conn: trillium::Conn| async move {
  let important_state: &MyState = trillium::conn_unwrap!(conn, conn.state());
  let ok_response = String::from(important_state.0);
  conn.ok(ok_response)
});

assert!(handler.get("/").status().is_none()); // we never reached the conn.ok line.
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
