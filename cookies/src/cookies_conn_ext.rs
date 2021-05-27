use cookie::{Cookie, CookieJar};
use trillium::Conn;

/**
Extension trait adding cookie capacities to [`Conn`].

Important: The [`CookiesHandler`](crate::CookiesHandler) must be
called before any of these functions can be called on a conn.
*/
pub trait CookiesConnExt {
    /// adds a cookie to the cookie jar and returns the conn
    fn with_cookie(self, cookie: Cookie<'_>) -> Self;
    /// gets a reference to the cookie jar
    fn cookies(&self) -> &CookieJar;
    /// gets a mutable reference to the cookie jar
    fn cookies_mut(&mut self) -> &mut CookieJar;
}

impl CookiesConnExt for Conn {
    fn cookies(&self) -> &CookieJar {
        self.state()
            .expect("Cookies handler must be executed before calling CookiesExt::cookies")
    }

    fn with_cookie(mut self, cookie: Cookie<'_>) -> Self {
        self.cookies_mut().add(cookie.into_owned());
        self
    }

    fn cookies_mut(&mut self) -> &mut CookieJar {
        self.state_mut()
            .expect("Cookies handler must be executed before calling CookiesExt::cookies_mut")
    }
}
