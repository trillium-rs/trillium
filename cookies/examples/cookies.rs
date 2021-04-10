use trillium::{sequence, Conn};
use trillium_cookies::{Cookie, Cookies, CookiesConnExt};

pub fn main() {
    env_logger::init();

    trillium_smol_server::run(sequence![Cookies, |conn: Conn| async move {
        if let Some(cookie_value) = conn.cookies().get("some_cookie") {
            println!("current cookie value: {}", cookie_value.value());
        }

        conn.ok("ok!").with_cookie(
            Cookie::build("some_cookie", "some-cookie-value")
                .path("/")
                .finish(),
        )
    }]);
}
