pub use cookie::*;
use myco::http_types::headers::{COOKIE, SET_COOKIE};
use myco::{async_trait, Conn, Grain};

pub struct Cookies;

#[async_trait]
impl Grain for Cookies {
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

pub trait CookiesConnExt {
    fn cookies(&self) -> &CookieJar;
    fn with_cookie(self, cookie: Cookie<'_>) -> Self;
    fn cookies_mut(&mut self) -> &mut CookieJar;
}

impl CookiesConnExt for Conn {
    fn cookies(&self) -> &CookieJar {
        self.state()
            .expect("Cookies grain must be executed before calling CookiesExt::cookies")
    }

    fn with_cookie(mut self, cookie: Cookie<'_>) -> Self {
        self.cookies_mut().add(cookie.into_owned());
        self
    }

    fn cookies_mut(&mut self) -> &mut CookieJar {
        self.state_mut()
            .expect("Cookies grain must be executed before calling CookiesExt::cookies_mut")
    }
}
