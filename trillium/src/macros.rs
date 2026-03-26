/// # Unwraps an `Result::Ok` or returns the `Conn` with a 500 status.
///
/// ```
/// use trillium::{Conn, conn_try};
/// use trillium_testing::TestServer;
///
/// # trillium_testing::block_on(async {
/// let handler = |mut conn: Conn| async move {
///     let request_body_string = conn_try!(conn.request_body_string().await, conn);
///     let u8: u8 = conn_try!(request_body_string.parse(), conn);
///     conn.ok(format!("received u8 as body: {}", u8))
/// };
///
/// let app = TestServer::new(handler).await;
/// app.post("/").with_body("not u8").await.assert_status(500);
/// app.post("/")
///     .with_body("10")
///     .await
///     .assert_ok()
///     .assert_body("received u8 as body: 10");
/// # });
/// ```
#[macro_export]
macro_rules! conn_try {
    ($expr:expr, $conn:expr $(,)?) => {
        match $expr {
            Ok(value) => value,
            Err(error) => {
                $crate::log::error!("{}:{} conn_try error: {}", file!(), line!(), error);
                return $conn.with_status(500).halt();
            }
        }
    };
}

/// # Unwraps an `Option::Some` or returns the `Conn`.
///
/// This is useful for gracefully exiting a `Handler` without
/// returning an error.
///
/// ```
/// use trillium::{Conn, State, conn_unwrap};
/// use trillium_testing::TestServer;
///
/// #[derive(Copy, Clone)]
/// struct MyState(&'static str);
/// let handler = |conn: trillium::Conn| async move {
///     let important_state: MyState = *conn_unwrap!(conn.state(), conn);
///     conn.ok(important_state.0)
/// };
///
/// # trillium_testing::block_on(async {
/// let app = TestServer::new(handler).await;
/// app.get("/").await.assert_status(404); // we never reached the conn.ok line.
///
/// let app2 = TestServer::new((State::new(MyState("hi")), handler)).await;
/// app2.get("/").await.assert_ok().assert_body("hi");
/// # });
/// ```
#[macro_export]
macro_rules! conn_unwrap {
    ($option:expr, $conn:expr $(,)?) => {
        match $option {
            Some(value) => value,
            None => return $conn,
        }
    };
}

/// # A convenience macro for logging the contents of error variants.
///
/// This is useful when there is no further action required to process the
/// error path, but you still want to record that it transpired
#[macro_export]
macro_rules! log_error {
    ($expr:expr_2021) => {
        if let Err(err) = $expr {
            $crate::log::error!("{}:{} {:?}", file!(), line!(), err);
        }
    };

    ($expr:expr_2021, $message:expr_2021) => {
        if let Err(err) = $expr {
            $crate::log::error!("{}:{} {} {:?}", file!(), line!(), $message, err);
        }
    };
}

/// # Macro for implementing Handler for simple newtypes that contain another handler.
///
/// ```
/// use trillium::{delegate_handler, State, Conn, conn_unwrap};
/// use trillium_testing::TestServer;
///
/// #[derive(Clone, Copy)]
/// struct MyState(usize);
/// struct MyHandler { handler: State<MyState> }
/// delegate_handler!(MyHandler => handler);
/// impl MyHandler {
/// fn new(n: usize) -> Self {
/// MyHandler { handler: State::new(MyState(n)) }
/// }
/// }
///
/// # trillium_testing::block_on(async {
/// let handler = (MyHandler::new(5), |conn: Conn| async move {
/// let MyState(n) = *conn_unwrap!(conn.state(), conn);
/// conn.ok(n.to_string())
/// });
/// let app = TestServer::new(handler).await;
/// app.get("/").await.assert_ok().assert_body("5");
/// # });
/// ```
///
/// ```
/// use trillium::{Conn, State, conn_unwrap, delegate_handler};
/// use trillium_testing::TestServer;
///
/// #[derive(Clone, Copy)]
/// struct MyState(usize);
/// struct MyHandler(State<MyState>);
/// delegate_handler!(MyHandler);
/// impl MyHandler {
///     fn new(n: usize) -> Self {
///         MyHandler(State::new(MyState(n)))
///     }
/// }
///
/// # trillium_testing::block_on(async {
/// let handler = (MyHandler::new(5), |conn: Conn| async move {
///     let MyState(n) = *conn_unwrap!(conn.state(), conn);
///     conn.ok(n.to_string())
/// });
/// let app = TestServer::new(handler).await;
/// app.get("/").await.assert_ok().assert_body("5");
/// # });
/// ```
#[macro_export]
macro_rules! delegate_handler {
    ($struct_name:ty) => {
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

    ($struct_name:ty => $target:ident) => {
        impl $crate::Handler for $struct_name {
            async fn run(&self, conn: $crate::Conn) -> $crate::Conn {
                use $crate::Handler;
                self.$target.run(conn).await
            }

            async fn init(&mut self, info: &mut $crate::Info) {
                use $crate::Handler;
                self.$target.init(info).await;
            }

            async fn before_send(&self, conn: $crate::Conn) -> $crate::Conn {
                use $crate::Handler;
                self.$target.before_send(conn).await
            }

            fn name(&self) -> std::borrow::Cow<'static, str> {
                use $crate::Handler;
                self.$target.name()
            }

            fn has_upgrade(&self, upgrade: &$crate::Upgrade) -> bool {
                use $crate::Handler;
                self.$target.has_upgrade(upgrade)
            }

            async fn upgrade(&self, upgrade: $crate::Upgrade) {
                use $crate::Handler;
                self.$target.upgrade(upgrade).await;
            }
        }
    };
}
