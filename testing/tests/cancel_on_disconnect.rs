use test_harness::test;
use trillium_testing::prelude::*;

#[test]
fn body_does_not_report_closure() {
    let handler = |mut conn: Conn| async move {
        let body = conn.request_body().await.read_string().await.unwrap();

        if conn.is_disconnected().await {
            return conn.with_status(500);
        }

        if conn
            .cancel_on_disconnect(futures_lite::future::yield_now())
            .await
            .is_none()
        {
            return conn.with_status(501);
        }

        conn.ok("ok").with_body(body)
    };

    assert_ok!(get("/").with_request_body("body").on(&handler), "body");
}
