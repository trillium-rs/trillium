use futures_lite::{StreamExt, stream};
use trillium_client::{Client, SseErrorKind};
use trillium_http::Status;
use trillium_sse::{Event as ServerEvent, SseConnExt};
use trillium_testing::client_config;

#[test]
fn round_trip_through_trillium_sse() {
    let handler = |conn: trillium::Conn| async move {
        let events = vec![
            ServerEvent::new("first"),
            ServerEvent::new("second line one\nsecond line two").with_type("custom"),
            ServerEvent::new("third"),
        ];
        conn.with_sse_stream(stream::iter(events))
    };

    let client = Client::new(client_config());

    trillium_testing::with_server(handler, move |url| async move {
        let mut events = client.get(url).into_sse().await?;

        let first = events.next().await.expect("first")?;
        assert_eq!(first.data(), "first");
        assert_eq!(first.event_type(), None);

        let second = events.next().await.expect("second")?;
        assert_eq!(second.data(), "second line one\nsecond line two");
        assert_eq!(second.event_type(), Some("custom"));

        let third = events.next().await.expect("third")?;
        assert_eq!(third.data(), "third");

        assert!(events.next().await.is_none());
        Ok(())
    });
}

#[test]
fn non_success_status_is_recoverable() {
    let handler = |conn: trillium::Conn| async move { conn.with_status(404).with_body("nope") };
    let client = Client::new(client_config());

    trillium_testing::with_server(handler, move |url| async move {
        let err = client
            .get(url)
            .into_sse()
            .await
            .expect_err("expected a 404");
        assert!(matches!(err.kind, SseErrorKind::Status(Status::NotFound)));

        let mut conn = trillium_client::Conn::from(err);
        assert_eq!(conn.response_body().read_string().await?, "nope");
        Ok(())
    });
}

#[test]
fn wrong_content_type_is_recoverable() {
    let handler = |conn: trillium::Conn| async move { conn.ok("data: not really sse\n\n") };
    let client = Client::new(client_config());

    trillium_testing::with_server(handler, move |url| async move {
        let err = client
            .get(url)
            .into_sse()
            .await
            .expect_err("expected wrong content-type");
        assert!(matches!(err.kind, SseErrorKind::UnexpectedContentType(_)));
        Ok(())
    });
}
