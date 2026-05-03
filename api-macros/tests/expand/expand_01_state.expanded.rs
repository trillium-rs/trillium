#[api(state)]
struct CurrentUser {
    name: String,
}
impl ::trillium_api::TryFromConn for CurrentUser {
    type Error = ();
    async fn try_from_conn(
        conn: &mut ::trillium::Conn,
    ) -> ::core::result::Result<Self, Self::Error> {
        conn.take_state::<Self>().ok_or(())
    }
}
