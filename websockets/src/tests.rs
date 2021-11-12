use super::connection_is_upgrade;
use trillium_testing::prelude::*;

#[test]
fn test_connection_is_upgrade() {
    let mut conn = get("/").on(&()).into();
    assert!(!connection_is_upgrade(&conn));

    conn.inner_mut()
        .request_headers_mut()
        .insert("connection", "keep-alive, Upgrade");
    assert!(connection_is_upgrade(&conn));

    conn.inner_mut()
        .request_headers_mut()
        .insert("connection", "upgrade");
    assert!(connection_is_upgrade(&conn));

    conn.inner_mut()
        .request_headers_mut()
        .insert("connection", "UPgrAde");
    assert!(connection_is_upgrade(&conn));

    conn.inner_mut()
        .request_headers_mut()
        .insert("connection", "UPgrAde, keep-alive");
    assert!(connection_is_upgrade(&conn));

    conn.inner_mut()
        .request_headers_mut()
        .insert("connection", "keep-alive");
    assert!(!connection_is_upgrade(&conn));
}
