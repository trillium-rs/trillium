use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, trillium_api::TryFromConn, trillium_api::Handler)]
#[api(body)]
struct Echo {
    payload: String,
}
