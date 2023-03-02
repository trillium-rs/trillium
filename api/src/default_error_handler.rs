use crate::{ApiConnExt, Error};
use serde_json::json;
use trillium::Conn;

pub(crate) fn handle_error(mut conn: Conn) -> Conn {
    if let Some(error) = conn.take_state::<Error>() {
        conn.with_json(&json!({ "error": &error }))
            .with_status(&error)
    } else {
        conn
    }
}
