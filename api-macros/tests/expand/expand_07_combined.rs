// Combined derive: TryFromConn + Handler with `state, clone`.
// `clone` is consumed by TryFromConn; `Handler` ignores it.

#[derive(Clone, trillium_api::TryFromConn, trillium_api::Handler)]
#[api(state, clone)]
struct CurrentUser {
    name: String,
}
