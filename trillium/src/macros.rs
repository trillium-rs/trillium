/**
# Unwraps an Result::Ok or returns the conn with a 500 status.

```
use trillium_testing::prelude::*;
use trillium::{Conn, conn_try};

let handler = |mut conn: Conn| async move {
  let request_body_string = conn_try!(conn.request_body_string().await, conn);
  let u8: u8 = conn_try!(request_body_string.parse(), conn);
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
    ($expr:expr, $conn:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => {
                $crate::log::error!("{}:{} conn_try error: {}", file!(), line!(), error);
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
use trillium_testing::prelude::*;
use trillium::{Conn, conn_unwrap, State};

#[derive(Copy, Clone)]
struct MyState(&'static str);
let handler = |conn: trillium::Conn| async move {
  let important_state: MyState = *conn_unwrap!(conn.state(), conn);
  conn.ok(important_state.0)
};

assert_not_handled!(get("/").on(&handler)); // we never reached the conn.ok line.

assert_ok!(
    get("/").on(&(State::new(MyState("hi")), handler)),
    "hi"
);
```
*/
#[macro_export]
macro_rules! conn_unwrap {
    ($option:expr, $conn:expr) => {
        match $option {
            Some(value) => value,
            None => return $conn,
        }
    };
}

/**
# A convenience macro for logging the contents of error variants.

This is useful when there is no further action required to process the
error path, but you still want to record that it transpired
*/
#[macro_export]
macro_rules! log_error {
    ($expr:expr) => {
        if let Err(err) = $expr {
            $crate::log::error!("{}:{} {:?}", file!(), line!(), err);
        }
    };

    ($expr:expr, $message:expr) => {
        if let Err(err) = $expr {
            $crate::log::error!("{}:{} {} {:?}", file!(), line!(), $message, err);
        }
    };
}

/**
# Macro for implementing Handler for simple newtypes that contain another handler.

```
use trillium::{delegate_handler, State, Conn, conn_unwrap};
#[derive(Clone, Copy)]
struct MyState(usize);
struct MyHandler(State<MyState>);
delegate_handler!(MyHandler);
impl MyHandler {
    fn new(n: usize) -> Self {
        MyHandler(State::new(MyState(n)))
    }
}

# use trillium_testing::prelude::*;

let handler = (MyHandler::new(5), |conn: Conn| async move {
    let MyState(n) = *conn_unwrap!(conn.state(), conn);
    conn.ok(n.to_string())
});
assert_ok!(get("/").on(&handler), "5");
```
*/
#[macro_export]
macro_rules! delegate_handler {
    ($struct_name:ty) => {
        #[$crate::async_trait]
        impl $crate::Handler for $struct_name {
            async fn run(&self, conn: $crate::Conn) -> $crate::Conn {
                use $crate::Handler;
                self.0.run(conn).await
            }

            async fn init(&mut self, info: &mut $crate::Info) {
                use $crate::Handler;
                self.0.init(info).await;
            }

            async fn before_send(&self, conn: $crate::Conn) -> $crate::Conn {
                use $crate::Handler;
                self.0.before_send(conn).await
            }

            fn name(&self) -> std::borrow::Cow<'static, str> {
                use $crate::Handler;
                self.0.name()
            }

            fn has_upgrade(&self, upgrade: &$crate::Upgrade) -> bool {
                use $crate::Handler;
                self.0.has_upgrade(upgrade)
            }

            async fn upgrade(&self, upgrade: $crate::Upgrade) {
                use $crate::Handler;
                self.0.upgrade(upgrade).await;
            }
        }
    };
}
