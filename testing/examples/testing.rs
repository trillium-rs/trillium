use trillium::{conn_try, Conn, Handler};
use trillium_logger::Logger;

async fn teapot(mut conn: Conn) -> Conn {
    let request_body = conn_try!(conn.request_body_string().await, conn);
    if request_body.is_empty() {
        conn.with_status(406).with_body("unacceptable!").halt()
    } else {
        conn.with_body(format!("request body was: {}", request_body))
            .with_status(418)
            .with_header(("server", "zojirushi"))
    }
}

fn application<R>() -> impl Handler<R> {
    (Logger::new(), teapot)
}

fn main() {
    trillium_smol::run(application());
}

#[cfg(test)]
mod tests {
    use super::{application, teapot};
    use trillium_testing::prelude::*;

    #[test]
    fn handler_sends_correct_headers_and_is_a_teapot() {
        let application = application();
        assert_response!(
            post("/").with_request_body("hello trillium!").on(&application),
            StatusCode::ImATeapot,
            "request body was: hello trillium!",
            "server" => "zojirushi",
            "content-length" => "33"

        );
    }

    #[test]
    fn we_can_also_test_the_individual_handler() {
        assert_body!(
            post("/").with_request_body("a different body").on(&teapot),
            "request body was: a different body"
        );
    }

    #[test]
    fn application_is_lemongrab_when_body_is_empty() {
        let application = application();
        assert_response!(
            post("/").on(&application),
            StatusCode::NotAcceptable,
            "unacceptable!"
        );
    }
}
