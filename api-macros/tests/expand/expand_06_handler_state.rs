#[derive(Clone, trillium_api::Handler)]
#[api(state)]
struct CurrentUser {
    name: String,
}
