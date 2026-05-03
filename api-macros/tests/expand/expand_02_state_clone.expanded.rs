#[api(state, clone)]
struct CurrentUser {
    name: String,
}
#[automatically_derived]
impl ::core::clone::Clone for CurrentUser {
    #[inline]
    fn clone(&self) -> CurrentUser {
        CurrentUser {
            name: ::core::clone::Clone::clone(&self.name),
        }
    }
}
impl ::trillium_api::TryFromConn for CurrentUser {
    type Error = ();
    async fn try_from_conn(
        conn: &mut ::trillium::Conn,
    ) -> ::core::result::Result<Self, Self::Error> {
        conn.state::<Self>().cloned().ok_or(())
    }
}
