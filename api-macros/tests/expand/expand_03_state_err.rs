#[derive(Default)]
struct ServerError;
impl trillium::Handler for ServerError {
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        conn.with_status(trillium::Status::InternalServerError).halt()
    }
}

#[derive(trillium_api::TryFromConn)]
#[api(state, err = ServerError)]
struct RequiredState(u32);
