use askama::Template;
use futures_lite::prelude::*;
use trillium::Conn;
use trillium_askama::AskamaConnExt;
use trillium_cookies::CookiesHandler;
use trillium_logger::DevLogger;
use trillium_proxy::{Proxy, Rustls, TcpStream};
use trillium_router::{routes, Router, RouterConnExt};
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};
use trillium_static_compiled::{include_dir, StaticCompiledHandler};
use trillium_websockets::{Message, WebSocket};

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

fn main() {
    env_logger::init();

    trillium_smol_server::run((
        DevLogger,
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), b"01234567890123456789012345678901123"),
        |conn: Conn| async move {
            let count = conn.session().get::<usize>("count").unwrap_or_default();
            conn.with_header(("request-count", count.to_string()))
                .with_session("count", count + 1)
        },
        // three different ways of defining a router. they
        // fall through, but normally an app would only use one of these
        // styles. they're all entirely equivalent
        routes![
            get "/hello" "hi",

            post "/" |mut conn: Conn| async move {
                let body = conn.request_body().await.read_string().await.unwrap();
                conn.ok(format!("request body: {}", body))
            },

            get "/template/:name" |conn: Conn| async move {
                if let Some(name) = conn.param("name").map(String::from) {
                    conn.render(HelloTemplate { name: &name })
                } else {
                    conn
                }
            },

        ],
        Router::new()
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
                        let output: String = input.chars().rev().collect();
                        ws.send_string(format!("{} | {}", &input, &output)).await;
                    }
                }),
            ),
        Router::build(|mut r| {
            r.get(
                "/httpbin/*",
                Proxy::<Rustls<TcpStream>>::new("https://httpbin.org"),
            );

            r.get(
                "*",
                StaticCompiledHandler::new(include_dir!("./public/")).with_index_file("index.html"),
            );
        }),
    ));
}
