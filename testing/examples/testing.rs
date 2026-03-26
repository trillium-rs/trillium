use trillium::{Conn, Handler, KnownHeaderName, conn_try};
use trillium_logger::Logger;

async fn teapot(mut conn: Conn) -> Conn {
    let request_body = conn_try!(conn.request_body_string().await, conn);
    if request_body.is_empty() {
        conn.with_status(406).with_body("unacceptable!").halt()
    } else {
        conn.with_body(format!("request body was: {request_body}"))
            .with_status(418)
            .with_response_header(KnownHeaderName::Server, "zojirushi")
    }
}

fn application() -> impl Handler {
    (Logger::new(), teapot)
}

fn main() {
    trillium_smol::run(application());
}

#[cfg(test)]
mod tests {
    use super::{application, teapot};
    use trillium_testing::{Status, TestServer, harness, test};

    #[test(harness)]
    async fn handler_sends_correct_headers_and_is_a_teapot() {
        let app = TestServer::new(application()).await;

        app.post("/")
            .with_body("hello trillium!")
            .await
            .assert_status(Status::ImATeapot)
            .assert_body("request body was: hello trillium!")
            .assert_headers([("server", "zojirushi"), ("content-length", "33")]);
    }

    #[test(harness)]
    async fn we_can_also_test_the_individual_handler() {
        let app = TestServer::new(teapot).await;
        app.post("/")
            .with_body("a different body")
            .await
            .assert_body("request body was: a different body");
    }

    #[test(harness)]
    async fn application_is_lemongrab_when_body_is_empty() {
        let app = TestServer::new(application()).await;
        app.post("/")
            .await
            .assert_status(Status::NotAcceptable)
            .assert_body("unacceptable!");
    }
}
