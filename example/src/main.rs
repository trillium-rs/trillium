use askama::Template;
use futures_lite::prelude::*;
use trillium::{Conn, Handler};
use trillium_askama::AskamaConnExt;
use trillium_cookies::CookiesHandler;
use trillium_logger::Logger;
use trillium_router::{Router, RouterConnExt};
use trillium_rustls::RustlsConnector;
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};
use trillium_smol::TcpConnector;
use trillium_static_compiled::{include_dir, StaticCompiledHandler};
use trillium_websockets::{Message, WebSocket};
type Proxy = trillium_proxy::Proxy<RustlsConnector<TcpConnector>>;

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

fn handler() -> impl Handler {
    (
        Logger::new(),
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), b"01234567890123456789012345678901123"),
        |conn: Conn| async move {
            let count = conn.session().get::<usize>("count").unwrap_or_default();
            conn.with_header(("request-count", count.to_string()))
                .with_session("count", count + 1)
        },
        Router::new()
            .get("/hello", "hi")
            .post("/", |mut conn: Conn| async move {
                let body = conn.request_body().await.read_string().await.unwrap();
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
                WebSocket::new(|mut ws| async move {
                    while let Some(Ok(Message::Text(input))) = ws.next().await {
                        log::info!("received message {:?}", &input);
                        let output: String = input.chars().rev().collect();
                        ws.send_string(format!("{} | {}", &input, &output)).await;
                    }
                }),
            )
            .get("/httpbin/*", Proxy::new("https://httpbin.org")),
        StaticCompiledHandler::new(include_dir!("./public")).with_index_file("index.html"),
    )
}

fn main() {
    env_logger::init();
    trillium_smol::run(handler());
}

#[cfg(test)]
mod test {
    use trillium_testing::{assert_ok, fluent::*};

    #[test]
    fn test_index() {
        let handler = super::handler();
        let mut conn = get("/").on(&handler);
        assert_ok!(&mut conn);
        let body = conn.take_body_string().unwrap();
        assert!(body.contains("<h1>Welcome to trillium!</h1>"));
    }

    #[test]
    fn test_hello_hi() {
        let handler = super::handler();
        assert_ok!(get("/hello").on(&handler), "hi");
    }

    #[test]
    fn test_post_index() {
        let handler = super::handler();
        assert_ok!(
            post("/").with_request_body("hey").on(&handler),
            "request body: hey"
        );
    }
}
