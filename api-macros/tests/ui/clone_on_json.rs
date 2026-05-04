#[derive(trillium_api::TryFromConn)]
#[api(json, clone)]
struct Bad {
    payload: String,
}

fn main() {}
