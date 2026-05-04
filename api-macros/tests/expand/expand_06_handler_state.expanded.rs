#[api(state)]
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
impl ::trillium::Handler for CurrentUser {
    async fn run(&self, conn: ::trillium::Conn) -> ::trillium::Conn {
        conn.with_state(<Self as ::core::clone::Clone>::clone(self))
    }
}
