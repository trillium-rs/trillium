use trillium::{Handler, Info};
use trillium_macros::Handler;
use trillium_testing::prelude::*;

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
    struct OuterHandler(InnerHandler);

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

    // just for dead code
    let _ = handler.not_handler;

    assert_ok!(get("/").on(&handler), "hi");
}
