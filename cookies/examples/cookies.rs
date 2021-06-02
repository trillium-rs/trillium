use trillium::Conn;
use trillium_cookies::{cookie::Cookie, CookiesConnExt, CookiesHandler};

pub fn main() {
    env_logger::init();

    trillium_smol::run((CookiesHandler::new(), |conn: Conn| async move {
        if let Some(cookie_value) = conn.cookies().get("some_cookie") {
            println!("current cookie value: {}", cookie_value.value());
        }

        conn.with_cookie(
            Cookie::build("some_cookie", "some-cookie-value")
                .path("/")
                .finish(),
        )
        .ok("ok!")
    }));
}
