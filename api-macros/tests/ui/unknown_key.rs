#[derive(trillium_api::TryFromConn)]
#[api(state, oops)]
struct Bad {
    name: String,
}

fn main() {}
