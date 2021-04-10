use askama::Template;
use trillium::{sequence, Conn};
use trillium_askama::AskamaConnExt;
use trillium_aws_lambda::LambdaConnExt;
use trillium_cookies::Cookies;
use trillium_logger::DevLogger;
use trillium_router::{Router, RouterConnExt};
use trillium_sessions::{CookieStore, SessionConnExt, Sessions};

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

fn main() {
    env_logger::init();
    trillium_aws_lambda::run(sequence![
        DevLogger,
        Cookies,
        Sessions::new(CookieStore::new(), b"01234567890123456789012345678901123",),
        |conn: Conn| async move {
            let count = conn.session().get::<usize>("count").unwrap_or_default();
            let request_id = conn.lambda_context().request_id.clone();
            conn.with_header(("request-count", count.to_string()))
                .with_header(("request-id", request_id))
                .with_session("count", count + 1)
        },
        Router::new()
            .get("/always-hi", "hi")
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
            .get("/", |conn: Conn| async move { conn.ok("hello world") })
            .get("/hello/:planet", |conn: Conn| async move {
                if let Some(planet) = conn.param("planet") {
                    let response = format!("hello, {}", planet);
                    conn.ok(response)
                } else {
                    conn
                }
            }),
    ])
}
