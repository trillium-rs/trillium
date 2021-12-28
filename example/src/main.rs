use askama::Template;
use futures_lite::prelude::*;
use std::time::Duration;
use trillium::{Conn, Handler};
use trillium_askama::AskamaConnExt;
use trillium_caching_headers::{
    CacheControlDirective::{MaxAge, Public},
    CachingHeadersExt,
};
use trillium_conn_id::log_formatter::conn_id;
use trillium_logger::apache_common;
use trillium_router::{Router, RouterConnExt};
use trillium_rustls::RustlsConnector;
use trillium_sessions::{MemoryStore, SessionConnExt};
use trillium_smol::TcpConnector;
use trillium_static_compiled::static_compiled;
use trillium_websockets::{Message, WebSocket, WebSocketConn};
type Proxy = trillium_proxy::Proxy<RustlsConnector<TcpConnector>>;

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

async fn request_count(conn: Conn) -> Conn {
    let count = conn.session().get::<usize>("count").unwrap_or_default();
    conn.with_header("request-count", count.to_string())
        .with_session("count", count + 1)
}

async fn with_cache_control(conn: Conn) -> Conn {
    conn.with_cache_control([MaxAge(Duration::from_secs(604800)), Public])
        .with_vary([trillium::KnownHeaderName::UserAgent])
}

fn app() -> impl Handler {
    (
        with_cache_control,
        trillium_logger::logger().with_formatter(apache_common(conn_id, "-")),
        trillium_compression::compression(),
        trillium_conn_id::conn_id(),
        trillium_method_override::method_override(),
        trillium_head::head(),
        trillium_caching_headers::caching_headers(),
        trillium_cookies::cookies(),
        trillium_sessions::sessions(MemoryStore::new(), b"01234567890123456789012345678901123"),
        request_count,
        router(),
        static_compiled!("$CARGO_MANIFEST_DIR/public").with_index_file("index.html"),
    )
}

fn router() -> impl Handler {
    Router::new()
        .get("/hello", "hi")
        .post("/", |mut conn: Conn| async move {
            let body = conn.request_body_string().await.unwrap();
            conn.ok(format!("request body: {}", body))
        })
        .get("/template/:name", |conn: Conn| async move {
            if let Some(name) = conn.param("name").map(String::from) {
                conn.render(HelloTemplate { name: &name })
            } else {
                conn
            }
        })
        .get("/hello/:planet", |conn: Conn| async move {
            if let Some(planet) = conn.param("planet") {
                let response = format!("hello, {}", planet);
                conn.ok(response)
            } else {
                conn
            }
        })
        .get(
            "/ws",
            WebSocket::new(|mut ws: WebSocketConn| async move {
                while let Some(Ok(Message::Text(input))) = ws.next().await {
                    log::info!("received message {:?}", &input);
                    let output: String = input.chars().rev().collect();
                    ws.send_string(format!("{} | {}", &input, &output)).await;
                }
            }),
        )
        .get("/httpbin/*", Proxy::new("https://httpbin.org"))
}

fn main() {
    env_logger::init();
    trillium_smol::run(app());
}

#[cfg(test)]
mod test {
    use trillium_testing::prelude::*;

    use super::app;

    #[test]
    fn test_index() {
        let app = app();
        let mut conn = get("/").on(&app);
        assert_ok!(&conn);
        assert_body_contains!(&mut conn, "<h1>Welcome to trillium!</h1>");
    }

    #[test]
    fn test_hello_hi() {
        let app = app();
        assert_ok!(get("/hello").on(&app), "hi");
    }

    #[test]
    fn test_post_index() {
        let app = app();
        assert_ok!(
            post("/").with_request_body("hey").on(&app),
            "request body: hey"
        );
    }

    #[test]
    fn test_askama_templating() {
        let app = app();
        assert_body_contains!(
            get("/template/trillium").on(&app),
            "<h1>hi there, trillium</h1>"
        );

        assert_body_contains!(
            get("/template/dear-reader").on(&app),
            "<h1>hi there, dear-reader</h1>"
        );
    }
}
