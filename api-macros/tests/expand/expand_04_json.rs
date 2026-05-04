use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, trillium_api::TryFromConn, trillium_api::Handler)]
#[api(json)]
struct Greeting {
    hello: String,
}
