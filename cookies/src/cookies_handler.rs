use cookie::{Cookie, CookieJar};
use trillium::http_types::headers::{COOKIE, SET_COOKIE};
use trillium::{async_trait, Conn, Handler};

/**
The trillium cookie handler. See crate level docs for an example. This
must run before any handlers access the cookie jar.
*/
#[derive(Clone, Copy, Debug, Default)]
pub struct CookiesHandler {
    // this is in order to force users to call CookiesHandler::new or
    // CookiesHandler::default, allowing us to add
    // customization/settings later without breaking existing usage
    _priv: (),
}

impl CookiesHandler {
    /// constructs a new cookies handler
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Handler for CookiesHandler {
    async fn run(&self, conn: Conn) -> Conn {
        let mut jar = CookieJar::new();

        if let Some(cookies) = conn.headers().get(COOKIE) {
            for cookie in cookies {
                for pair in cookie.as_str().split(';') {
                    if let Ok(cookie) = Cookie::parse_encoded(String::from(pair)) {
                        jar.add_original(cookie);
                    }
                }
            }
        }

        conn.with_state(jar)
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(jar) = conn.take_state::<CookieJar>() {
            let headers = conn.headers_mut();

            for cookie in jar.delta() {
                headers.append(SET_COOKIE, cookie.encoded().to_string());
            }
        }

        conn
    }
}
