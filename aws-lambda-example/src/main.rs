use askama::Template;
use myco::{Conn, Sequence};
use myco_askama::AskamaConnExt;
use myco_aws_lambda::LambdaConnExt;
use myco_cookies::Cookies;
use myco_logger::DevLogger;
use myco_router::{Router, RouterConnExt};
use myco_sessions::{CookieStore, SessionConnExt, Sessions};

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

fn main() {
    env_logger::init();
    myco_aws_lambda::run(
        Sequence::new()
            .and(DevLogger)
            .and(Cookies)
            .and(Sessions::new(
                CookieStore::new(),
                b"01234567890123456789012345678901123",
            ))
            .and(|conn: Conn| async move {
                println!("lambda context: {:?}", conn.lambda_context());
                let count = conn.session().get::<usize>("count").unwrap_or_default();
                conn.send_header("request-count", count.to_string())
                    .with_session("count", count + 1)
            })
            .and(
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
            ),
    )
}
