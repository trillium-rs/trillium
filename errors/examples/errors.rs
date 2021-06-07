use std::{error::Error, future::Future, pin::Pin};
use trillium::{conn_unwrap, Conn, Handler};
use trillium_errors::{ErrorHandler, ErrorResult, Errorable};
use trillium_logger::Logger;
use trillium_router::{Router, RouterConnExt};
async fn hello_world(conn: Conn) -> Conn {
    conn.ok("hello world!")
}

async fn hello_name(conn: Conn) -> Conn {
    let name = conn_unwrap!(conn, conn.param("name"));
    let body = format!("hello, {}!", name);
    conn.ok(body)
}

async fn not_found(conn: Conn) -> Conn {
    let body = format!("Uh oh, I don't have a route for {}", conn.path());
    conn.with_body(body).with_status(404)
}

#[derive(Debug)]
enum MyError {
    A(&'static str),
    B,
}

#[trillium::async_trait]
impl Errorable for MyError {
    async fn run(self, conn: Conn) -> Conn {
        conn.with_body(format!("{:?}", self))
            .with_status(500)
            .halt()
    }
}

fn eh<'a>(conn: &'a mut Conn) -> ErrorResult<'a, MyError> {
    Box::pin(async move {
        conn.set_status(200);
        conn.set_body("hello");
        conn.set_halted(true);
        Err(MyError::A("aaaa"))
    })
}

fn application() -> impl Handler {
    (
        Logger::new(),
        Router::new()
            .get("/", hello_world)
            .get("/eh", ErrorHandler::new(eh))
            .get("/hello/:name", hello_name)
            .get("/panic", |_| async move { panic!("np") }),
        not_found,
    )
}

fn main() {
    trillium_smol::run(application())
}

// trait EH: Send + Sync {
//     fn run<'a>(
//         &self,
//         conn: &'a mut Conn,
//     ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>;
// }

// impl<Fun, Fut> EH for Fun
// where
//     Fun: for<'a> Fn(&'a mut Conn) ->  + Send + Sync,
//     Fut<'a>: Future<Output = Result<(), ()>> + Send + 'a,
// {
//     fn run(&self, conn: &'a mut Conn) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>> {
//         Box::pin(self(conn))
//     }
// }

// struct EHH<E>(E);
// #[trillium::async_trait]
// impl<E: EH + 'static> Handler for EHH<E> {
//     async fn run(&self, mut conn: Conn) -> Conn {
//         let _ = self.0.run(&mut conn).await;
//         conn
//     }
// }

#[cfg(test)]
mod tests {
    use super::application;
    use trillium_testing::prelude::*;

    #[test]
    fn says_hello_world() {
        assert_ok!(get("/").on(&application()), "hello world!")
    }

    #[test]
    fn says_hello_name() {
        assert_ok!(
            get("/hello/trillium").on(&application()),
            "hello, trillium!"
        );
        assert_ok!(get("/hello/rust").on(&application()), "hello, rust!");
    }

    #[test]
    fn other_routes_are_not_found() {
        assert_response!(
            get("/not/found").on(&application()),
            404,
            "Uh oh, I don't have a route for /not/found"
        )
    }
}
