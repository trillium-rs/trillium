use askama::Template;
use futures_lite::prelude::*;
use trillium::{Conn, FileSystem, Handler, Runtime};
use trillium_askama::AskamaConnExt;
use trillium_cookies::CookiesHandler;
use trillium_logger::{dev_formatter, Logger};
use trillium_proxy::{Connector, Proxy};
use trillium_router::{Router, RouterConnExt};
use trillium_rustls::RustlsConnector;
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};
use trillium_static_compiled::{include_dir, StaticCompiledHandler};
use trillium_websockets::{Message, WebSocket};

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

async fn request_count(conn: Conn) -> Conn {
    let count = conn.session().get::<usize>("count").unwrap_or_default();
    conn.with_header(("request-count", count.to_string()))
        .with_session("count", count + 1)
}

fn request_count_formatter(conn: &Conn, _color: bool) -> String {
    conn.session().get_raw("count").unwrap_or_default().clone()
}

fn router<R>() -> impl Handler<R>
where
    R: Runtime + FileSystem + Connector,
{
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
            WebSocket::new(|mut ws| async move {
                while let Some(Ok(Message::Text(input))) = ws.next().await {
                    log::info!("received message {:?}", &input);
                    let output: String = input.chars().rev().collect();
                    ws.send_string(format!("{} | {}", &input, &output)).await;
                }
            }),
        )
        .any(
            "/httpbin/*",
            Proxy::new("https://httpbin.org").with_connector(RustlsConnector::new()),
        )
}

fn app<R>() -> impl Handler<R>
where
    R: Runtime + FileSystem + Connector,
{
    (
        Logger::new().with_formatter((dev_formatter, " count=", request_count_formatter)),
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), b"01234567890123456789012345678901123"),
        request_count,
        router(),
        StaticCompiledHandler::new(include_dir!("./public")).with_index_file("index.html"),
    )
}

fn main() {
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
