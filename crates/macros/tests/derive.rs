#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use trillium::{Conn, Handler, Info, Status::Ok};
use trillium_macros::Handler;
use trillium_testing::prelude::*;

fn assert_handler(_: impl Handler) {}

#[test]
fn full_lifecycle() {
    struct InnerHandler {
        init: bool,
    }

    #[trillium::async_trait]
    impl Handler for InnerHandler {
        async fn run(&self, conn: Conn) -> Conn {
            conn.ok("run")
        }

        async fn init(&mut self, info: &mut Info) {
            self.init = true;
            *info.server_description_mut() = "inner handler took over".into();
        }

        async fn before_send(&self, conn: Conn) -> Conn {
            conn.with_header("before-send", "before-send")
        }

        fn name(&self) -> std::borrow::Cow<'static, str> {
            "inner handler".into()
        }
    }

    #[derive(Handler)]
    struct OuterHandler<X>(X);

    block_on(async {
        let mut info = Info::default();
        let mut handler = OuterHandler(InnerHandler { init: false });

        handler.init(&mut info).await;
        assert_eq!(info.server_description(), "inner handler took over");
        assert!(handler.0.init);
        assert_ok!(get("/").run_async(&handler).await, "run", "before-send" => "before-send");
        assert_eq!(handler.name(), "OuterHandler (inner handler)");
    });
}

#[test]
fn unnamed_1() {
    #[derive(Handler, Clone)]
    struct Foo(String);

    let handler = Foo(String::from("hi"));

    assert_ok!(get("/").on(&handler), "hi");
}

#[test]
fn unnamed_2() {
    #[derive(Handler)]
    struct Foo(&'static str, #[handler] &'static str);

    let handler = Foo("not-run", "hi");

    assert_ok!(get("/").on(&handler), "hi");
}

#[test]
fn named_1() {
    #[derive(Handler, Clone)]
    struct Foo {
        handler: String,
    }

    let handler = Foo {
        handler: String::from("hi"),
    };

    assert_ok!(get("/").on(&handler), "hi");
}

#[test]
fn named_2() {
    #[derive(Handler)]
    struct Foo {
        #[handler]
        handler: String,
        not_handler: (),
    }

    let handler = Foo {
        handler: String::from("hi"),
        not_handler: (),
    };

    assert_ok!(get("/").on(&handler), "hi");
}

#[test]
fn unnamed_generic() {
    #[derive(Handler)]
    struct Foo<X>(X);
    assert_handler(Foo(Ok));

    #[derive(Handler)]
    struct Bar<X, Y>((X, Y));

    assert_handler(Bar((Ok, "yes")));
    let handler = Bar((Ok, "yes"));

    assert_ok!(get("/").on(&handler));

    #[derive(Handler)]
    struct Hard<X: Copy, Y: Clone, Z>(X, #[handler] (Y, Z));
    assert_handler(Hard("hello", (Ok, "world")));
}

#[test]
fn named_generic() {
    #[derive(Handler)]
    struct Foo<X> {
        x: X,
    }
    assert_handler(Foo { x: Ok });

    #[derive(Handler)]
    struct Bar<X, Y> {
        thing: (X, Y),
    }
    assert_handler(Bar { thing: (Ok, "yes") });

    let handler = Bar { thing: (Ok, "yes") };
    assert_ok!(get("/").on(&handler));

    #[derive(Handler)]
    struct Hard<X: Copy, Y: Clone, Z> {
        x: X,
        #[handler]
        y_and_z: (Y, Z),
    }

    assert_handler(Hard {
        x: "hello",
        y_and_z: (Ok, "world"),
    });
}

#[test]
fn overriding_name() {
    #[derive(Handler)]
    struct CustomName {
        #[handler(except = name)]
        inner: &'static str,
    }

    impl CustomName {
        fn name(&self) -> std::borrow::Cow<'static, str> {
            format!("custom name ({})", &self.inner).into()
        }
    }

    let handler = CustomName { inner: "handler" };
    assert_eq!(trillium::Handler::name(&handler), "custom name (handler)");
    assert_handler(handler);
}

#[test]
fn overriding_run_and_before_send() {
    #[derive(Handler)]
    struct Counter {
        #[handler(except = [run, before_send])]
        inner: &'static str,
        count: AtomicUsize,
    }

    impl Counter {
        async fn run(&self, conn: Conn) -> Conn {
            self.count.fetch_add(1, Ordering::Relaxed);
            let conn = self.inner.run(conn).await;
            self.count.fetch_sub(1, Ordering::Relaxed);
            conn
        }

        async fn before_send(&self, conn: Conn) -> Conn {
            self.count.fetch_add(1, Ordering::Relaxed);
            let conn = self.inner.before_send(conn).await;
            self.count.fetch_sub(1, Ordering::Relaxed);
            conn
        }
    }

    let handler = Counter {
        inner: "handler",
        count: Default::default(),
    };

    assert_handler(handler);
}
