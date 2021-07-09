use trillium_testing::prelude::*;

fn app() -> impl trillium::Handler {
    |conn: trillium::Conn| async move {
        let response = tokio::task::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            "successfully spawned a task"
        })
        .await
        .unwrap();
        conn.ok(response)
    }
}

#[cfg(feature = "tokio")]
#[test]
fn smoke() {
    let app = app();
    assert_ok!(get("/").on(&app), "successfully spawned a task");
}
