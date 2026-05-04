struct ServerError;
#[automatically_derived]
impl ::core::default::Default for ServerError {
    #[inline]
    fn default() -> ServerError {
        ServerError {}
    }
}
impl trillium::Handler for ServerError {
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        conn.with_status(trillium::Status::InternalServerError).halt()
    }
}
#[api(state, err = ServerError)]
struct RequiredState(u32);
impl ::trillium_api::TryFromConn for RequiredState {
    type Error = ServerError;
    async fn try_from_conn(
        conn: &mut ::trillium::Conn,
    ) -> ::core::result::Result<Self, Self::Error> {
        conn.take_state::<Self>()
            .ok_or_else(<ServerError as ::core::default::Default>::default)
    }
}
