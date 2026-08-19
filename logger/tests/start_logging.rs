//! Coverage for the opt-in request-start line.

mod common;

use common::{CollectTarget, teapot};
use trillium::BoxedHandler;
use trillium_logger::{ColorMode, LogFormatter, Logger, formatters::method};
use trillium_testing::{TestServer, harness, test};

async fn server_with(
    logger: Logger<impl LogFormatter, impl LogFormatter>,
    target: CollectTarget,
) -> TestServer<BoxedHandler> {
    let logger = logger
        .with_target(target)
        .with_color_mode(ColorMode::Off)
        .without_init_message();
    TestServer::new(BoxedHandler::new((logger, teapot))).await
}

#[test(harness)]
async fn disabled_by_default() {
    let target = CollectTarget::default();
    let app = server_with(Logger::new().with_formatter(method), target.clone()).await;
    app.get("/").await;
    assert_eq!(target.next().await, "GET");
    assert_eq!(target.try_next(), None);
}

#[test(harness)]
async fn default_start_line() {
    let target = CollectTarget::default();
    let logger = Logger::new().with_formatter(method).with_start_logging();
    let app = server_with(logger, target.clone()).await;
    app.get("/widgets?id=1").await;
    assert_eq!(target.next().await, "Started HTTP/1.1 GET /widgets?id=1");
    assert_eq!(target.next().await, "GET");
}

#[test(harness)]
async fn custom_start_formatter_implies_start_logging() {
    let target = CollectTarget::default();
    let logger = Logger::new()
        .with_formatter(method)
        .with_start_formatter(("go ", method));
    let app = server_with(logger, target.clone()).await;
    app.get("/").await;
    assert_eq!(target.next().await, "go GET");
    assert_eq!(target.next().await, "GET");
}
