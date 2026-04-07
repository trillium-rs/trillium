use trillium::{Conn, KnownHeaderName};
use trillium_testing::{TestServer, harness, test};

#[test(harness)]
async fn assert_header_with() {
    let app = TestServer::new(|conn: Conn| async move {
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
    let app = TestServer::new(|conn: Conn| async move { conn.ok("ok") }).await;

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
    let app = TestServer::new(|conn: Conn| async move { conn.ok("ok") }).await;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: () = futures_lite::future::block_on(async {
            app.get("/").await.assert_headers([("x-missing", "")]);
        });
    }));

    assert!(result.is_err());
}

#[test(harness)]
async fn assert_headers() {
    let app = TestServer::new(|conn: Conn| async move {
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
    let app = TestServer::new(|conn: Conn| async move { conn.ok("hello from the body") }).await;

    app.get("/").await.assert_ok().assert_body_with(|body| {
        assert!(body.contains("hello"));
        assert_eq!(body.len(), 19);
    });
}

#[test(harness)]
async fn assert_body_with_returns_self() {
    let app =
        TestServer::new(|conn: Conn| async move { conn.with_status(201).with_body("created") })
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

    let app = TestServer::new(|mut conn: Conn| async move {
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
    let app = TestServer::new(|conn: Conn| async move { conn.ok("ok") }).await;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: () = futures_lite::future::block_on(async {
            app.get("/").await.assert_state_with(|_state: &String| {});
        });
    }));

    assert!(result.is_err());
}

#[test(harness)]
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
async fn assert_json_body_with() {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Response {
        message: String,
        code: u32,
    }

    let app = TestServer::new(|conn: Conn| async move {
        conn.with_response_header("content-type", "application/json")
            .with_body(trillium_testing::json!({"message": "hello", "code": 42}).to_string())
            .with_status(200)
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
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
async fn assert_json_body_with_invalid_json_panics() {
    let app = TestServer::new(|conn: Conn| async move { conn.with_body("not json") }).await;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: () = futures_lite::future::block_on(async {
            app.get("/")
                .await
                .assert_json_body_with(|_: &trillium_testing::Value| {});
        });
    }));

    assert!(result.is_err());
}

#[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
#[test(harness)]
async fn assert_json_body() {
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct ResponseBody {
        name: String,
    }

    let app = TestServer::new(|conn: Conn| async move {
        conn.with_status(200)
            .with_body(r#"{ "name": "trillium" }"#)
            .with_response_header("content-type", "application/json")
    })
    .await;

    app.get("/")
        .await
        .assert_status(200)
        .assert_json_body(&ResponseBody {
            name: "trillium".into(),
        })
        .assert_json_body(&trillium_testing::json!({"name":"trillium"}));
}

#[test(harness)]
async fn chaining_multiple_with_assertions() {
    let app = TestServer::new(|conn: Conn| async move {
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

#[test(harness)]
#[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
async fn json_request_body_builder() {
    #[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq)]
    struct Server {
        name: String,
    }

    let app = TestServer::new(|mut conn: Conn| async move {
        let received_body = conn.request_body_string().await.unwrap();
        let server: Server = trillium_testing::from_json_str(&received_body).unwrap();
        conn.with_status(200)
            .with_body(format!("server name was: {}", server.name))
    })
    .await;

    app.post("/")
        .with_json_body(&Server {
            name: "trillium".into(),
        })
        .await
        .assert_status(200)
        .assert_body("server name was: trillium");
}

// === HTTP Verbs ===

#[test(harness)]
async fn http_verbs_put_delete_patch() {
    let app = TestServer::new(|conn: Conn| async move {
        let method = conn.method().to_string();
        conn.ok(method)
    })
    .await;

    app.put("/").await.assert_ok().assert_body("PUT");
    app.delete("/").await.assert_ok().assert_body("DELETE");
    app.patch("/").await.assert_ok().assert_body("PATCH");
}

// === Request Headers ===

#[test(harness)]
async fn with_request_header() {
    let app = TestServer::new(|conn: Conn| async move {
        let header = conn
            .request_headers()
            .get_str("x-custom")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "missing".to_string());
        conn.ok(header)
    })
    .await;

    app.get("/")
        .with_request_header("x-custom", "test-value")
        .await
        .assert_ok()
        .assert_body("test-value");
}

#[test(harness)]
async fn with_request_headers() {
    let app = TestServer::new(|conn: Conn| async move {
        let h1 = conn
            .request_headers()
            .get_str("x-one")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());
        let h2 = conn
            .request_headers()
            .get_str("x-two")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());
        conn.ok(format!("{} {}", h1, h2))
    })
    .await;

    app.get("/")
        .with_request_headers([("x-one", "first"), ("x-two", "second")])
        .await
        .assert_ok()
        .assert_body("first second");
}

#[test(harness)]
async fn without_request_header() {
    let app = TestServer::new(|conn: Conn| async move {
        let present = conn.request_headers().get_str("user-agent").is_some();
        conn.ok(if present { "yes" } else { "no" })
    })
    .await;

    // Default user-agent is present
    app.get("/").await.assert_body("yes");

    // After removing it
    app.get("/")
        .without_request_header("user-agent")
        .await
        .assert_body("no");
}

// === Request Body ===

#[test(harness)]
async fn with_body() {
    let app = TestServer::new(|mut conn: Conn| async move {
        let body = conn.request_body_string().await.unwrap_or_default();
        conn.ok(format!("received: {}", body))
    })
    .await;

    app.post("/")
        .with_body("hello from request")
        .await
        .assert_ok()
        .assert_body("received: hello from request");
}

// === Peer IP ===

#[test(harness)]
async fn with_peer_ip() {
    use std::net::IpAddr;

    let app = TestServer::new(|conn: Conn| async move {
        let peer_ip = conn
            .peer_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "none".to_string());
        conn.ok(peer_ip)
    })
    .await;

    app.get("/")
        .with_peer_ip("192.168.1.100".parse::<IpAddr>().unwrap())
        .await
        .assert_ok()
        .assert_body("192.168.1.100");
}

// === State Accessors ===

#[test(harness)]
async fn assert_state() {
    #[derive(PartialEq, Debug)]
    struct Counter(i32);

    let app = TestServer::new(|mut conn: Conn| async move {
        conn.insert_state(Counter(42));
        conn.ok("ok")
    })
    .await;

    app.get("/").await.assert_state(Counter(42));
}

#[test(harness)]
async fn assert_no_state() {
    let app = TestServer::new(|conn: Conn| async move { conn.ok("ok") }).await;

    app.get("/").await.assert_no_state::<String>();
}

#[test(harness)]
async fn state_accessor() {
    #[derive(Debug)]
    struct Data(String);

    let app = TestServer::new(|mut conn: Conn| async move {
        conn.insert_state(Data("test data".into()));
        conn.ok("ok")
    })
    .await;

    let result = app.get("/").await;
    assert!(result.state::<Data>().is_some());
    assert_eq!(result.state::<Data>().unwrap().0, "test data");
}

// === Body and Status Accessors ===

#[test(harness)]
async fn status_accessor() {
    let app =
        TestServer::new(|conn: Conn| async move { conn.with_status(201).with_body("created") })
            .await;

    let result = app.get("/").await;
    assert_eq!(result.status(), trillium::Status::Created);
}

#[test(harness)]
async fn body_bytes_accessor() {
    let app = TestServer::new(|conn: Conn| async move { conn.ok("binary data") }).await;

    let result = app.get("/").await;
    let bytes = result.body_bytes();
    assert_eq!(bytes, b"binary data");
}

// === Header Assertions ===

#[test(harness)]
async fn assert_header() {
    let app = TestServer::new(|conn: Conn| async move {
        conn.ok("ok")
            .with_response_header("content-type", "text/plain")
    })
    .await;

    app.get("/")
        .await
        .assert_header("content-type", "text/plain");
}

#[test(harness)]
async fn assert_no_header() {
    let app = TestServer::new(|conn: Conn| async move { conn.ok("ok") }).await;

    app.get("/").await.assert_no_header("x-custom-header");
}

#[test(harness)]
async fn assert_body_contains() {
    let app = TestServer::new(|conn: Conn| async move {
        conn.ok("the quick brown fox jumps over the lazy dog")
    })
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_body_contains("brown fox")
        .assert_body_contains("lazy dog");
}

// === Host and Base Configuration ===

#[test(harness)]
async fn with_host() {
    let app = TestServer::new(|conn: Conn| async move {
        let host = conn
            .request_headers()
            .get_str("host")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "no-host".to_string());
        conn.ok(host)
    })
    .await;

    app.with_host("example.com")
        .get("/")
        .await
        .assert_ok()
        .assert_body("example.com");
}

#[test(harness)]
async fn set_host() {
    let mut app = TestServer::new(|conn: Conn| async move {
        let host = conn
            .request_headers()
            .get_str("host")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "no-host".to_string());
        conn.ok(host)
    })
    .await;

    app.set_host("custom.local");
    app.get("/").await.assert_ok().assert_body("custom.local");
}

#[test(harness)]
async fn with_base() {
    let app = TestServer::new(|conn: Conn| async move {
        let path = conn.path().to_string();
        conn.ok(path)
    })
    .await;

    app.with_base("https://api.example.com")
        .get("/users")
        .await
        .assert_ok()
        .assert_body("/users");
}

#[test(harness)]
async fn set_base() {
    let mut app = TestServer::new(|conn: Conn| async move {
        let path = conn.path().to_string();
        conn.ok(path)
    })
    .await;

    app.set_base("https://api.example.com");
    app.get("/items").await.assert_body("/items");
}

// === Blocking API ===

#[test]
fn new_blocking() {
    let app = TestServer::new_blocking(|conn: Conn| async move { conn.ok("blocking") });

    let result = app.get("/").block();
    assert_eq!(result.status(), trillium::Status::Ok);
    assert_eq!(result.body(), "blocking");
}

#[test]
fn block_method() {
    let app = TestServer::new_blocking(|conn: Conn| async move { conn.ok("blocked") });

    let result = app.get("/").block();
    assert_eq!(result.body(), "blocked");
}

// === Server and Handler Access ===

#[test(harness)]
async fn handler_borrow() {
    let handler = |conn: Conn| async move { conn.ok("handler") };
    let app = TestServer::new(handler).await;

    // Just verify we can borrow the handler without panicking
    let _ = app.handler();
    app.get("/").await.assert_ok();
}

#[test(harness)]
async fn shared_state() {
    use trillium::{Handler, Info};

    struct StateProvider;

    impl Handler for StateProvider {
        async fn run(&self, conn: Conn) -> Conn {
            conn.ok("ok")
        }

        async fn init(&mut self, info: &mut Info) {
            info.insert_shared_state("shared value".to_string());
        }
    }

    let app = TestServer::new(StateProvider).await;
    let state = app.shared_state::<String>();
    assert_eq!(state.map(|s| s.as_str()), Some("shared value"));
}
