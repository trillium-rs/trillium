#[derive(trillium_api::TryFromConn)]
#[api(state)]
struct CurrentUser {
    name: String,
}
