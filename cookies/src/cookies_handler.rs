use cookie::{Cookie, CookieJar};
use trillium::{async_trait, Conn, Handler, HeaderValue, HeaderValues, KnownHeaderName};

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

        if let Some(cookies) = conn.headers().get_values(KnownHeaderName::Cookie) {
            for cookie in cookies.iter().filter_map(HeaderValue::as_str) {
                for pair in cookie.split(';') {
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
            conn.headers_mut().append(
                KnownHeaderName::SetCookie,
                jar.delta()
                    .map(|cookie| cookie.encoded().to_string())
                    .collect::<HeaderValues>(),
            );
        }

        conn
    }
}

/// Alias for CookiesHandler::new()
pub fn cookies() -> CookiesHandler {
    CookiesHandler::new()
}
