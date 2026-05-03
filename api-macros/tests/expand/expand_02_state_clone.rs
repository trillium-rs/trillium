#[derive(Clone, trillium_api::TryFromConn)]
#[api(state, clone)]
struct CurrentUser {
    name: String,
}
