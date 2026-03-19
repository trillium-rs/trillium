use trillium::{Conn, KnownHeaderName};
use trillium_testing::{TestHandler, harness, test};

#[test(harness)]
async fn assert_header_with() {
    let app = TestHandler::new(|conn: Conn| async move {
        conn.with_response_header("x-test", "hello world").ok("ok")
    })
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_header_with("x-test", |value| {
            assert_eq!(value, "hello world");
            assert!(value.as_str().unwrap().starts_with("hello"));
        });
}

#[test(harness)]
async fn assert_header_with_missing_header_panics() {
    let app = TestHandler::new(|conn: Conn| async move { conn.ok("ok") }).await;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: () = futures_lite::future::block_on(async {
            app.get("/")
                .await
                .assert_header_with("x-missing", |_value| {});
        });
    }));

    assert!(result.is_err());
}

#[test(harness)]
async fn assert_headers_missing_header() {
    let app = TestHandler::new(|conn: Conn| async move { conn.ok("ok") }).await;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: () = futures_lite::future::block_on(async {
            app.get("/").await.assert_headers([("x-missing", "")]);
        });
    }));

    assert!(result.is_err());
}

#[test(harness)]
async fn assert_headers() {
    let app = TestHandler::new(|conn: Conn| async move {
        conn.ok("ok")
            .with_response_header(KnownHeaderName::Width, "wide")
            .with_response_header(KnownHeaderName::Warning, "strange headers")
    })
    .await;

    app.get("/")
        .await
        .assert_headers([("width", "wide"), ("warning", "strange headers")])
        .assert_headers([
            (KnownHeaderName::Width, "wide"),
            (KnownHeaderName::Warning, "strange headers"),
        ]);
}

#[test(harness)]
async fn assert_body_with() {
    let app = TestHandler::new(|conn: Conn| async move { conn.ok("hello from the body") }).await;

    app.get("/").await.assert_ok().assert_body_with(|body| {
        assert!(body.contains("hello"));
        assert_eq!(body.len(), 19);
    });
}

#[test(harness)]
async fn assert_body_with_returns_self() {
    let app =
        TestHandler::new(|conn: Conn| async move { conn.with_status(201).with_body("created") })
            .await;

    app.get("/")
        .await
        .assert_status(201)
        .assert_body_with(|body| {
            assert_eq!(body, "created");
        })
        .assert_body("created");
}

#[test(harness)]
async fn assert_state_with() {
    #[derive(PartialEq, Debug)]
    struct TestState(String);

    let app = TestHandler::new(|mut conn: Conn| async move {
        conn.insert_state(TestState("test value".to_string()));
        conn.ok("ok")
    })
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_state_with(|state: &TestState| {
            assert_eq!(state.0, "test value");
            assert!(state.0.contains("test"));
        });
}

#[test(harness)]
async fn assert_state_with_missing_state_panics() {
    let app = TestHandler::new(|conn: Conn| async move { conn.ok("ok") }).await;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: () = futures_lite::future::block_on(async {
            app.get("/").await.assert_state_with(|_state: &String| {});
        });
    }));

    assert!(result.is_err());
}

#[test(harness)]
#[cfg(feature = "serde_json")]
async fn assert_json_body_with() {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Response {
        message: String,
        code: u32,
    }

    let app = TestHandler::new(|conn: Conn| async move {
        conn.with_response_header("content-type", "application/json")
            .with_body(serde_json::json!({"message": "hello", "code": 42}).to_string())
    })
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_json_body_with(|response: &Response| {
            assert_eq!(response.message, "hello");
            assert_eq!(response.code, 42);
        });
}

#[test(harness)]
#[cfg(feature = "serde_json")]
async fn assert_json_body_with_invalid_json_panics() {
    let app = TestHandler::new(|conn: Conn| async move { conn.with_body("not json") }).await;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: () = futures_lite::future::block_on(async {
            app.get("/")
                .await
                .assert_json_body_with(|_: &serde_json::Value| {});
        });
    }));

    assert!(result.is_err());
}

#[test(harness)]
async fn chaining_multiple_with_assertions() {
    let app = TestHandler::new(|conn: Conn| async move {
        conn.with_status(200)
            .with_response_header("x-id", "123")
            .with_response_header("x-count", "5")
            .with_body("result data")
    })
    .await;

    app.get("/")
        .await
        .assert_status(200)
        .assert_header_with("x-id", |id| {
            assert_eq!(id, "123");
        })
        .assert_header_with("x-count", |count| {
            assert_eq!(count.as_str().unwrap().parse::<u32>().unwrap(), 5);
        })
        .assert_body_with(|body| {
            assert!(body.contains("result"));
        });
}
