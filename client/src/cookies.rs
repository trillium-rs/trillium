use crate::{async_trait, ClientHandler, Conn, KnownHeaderName, Result};
use async_lock::RwLock;
use cookie_store::CookieStore;
use trillium_http::HeaderValue;

/// handler for client cookies
#[derive(Debug, Default)]
pub struct Cookies {
    store: RwLock<CookieStore>,
}

impl Cookies {
    /// constructs a new cookies handler
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ClientHandler for Cookies {
    async fn before(&self, conn: &mut Conn) -> Result<()> {
        let guard = self.store.read().await;
        let mut matches = guard.matches(conn.url());
        matches.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
        let values = matches
            .iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<_>>()
            .join("; ");
        conn.request_headers()
            .append(KnownHeaderName::Cookie, values);
        Ok(())
    }

    async fn after(&self, conn: &mut Conn) -> Result<()> {
        if let Some(set_cookies) = conn
            .response_headers()
            .get_values(KnownHeaderName::SetCookie)
        {
            let mut cookie_store = self.store.write().await;
            for cookie in set_cookies.iter().filter_map(HeaderValue::as_str) {
                match cookie_store.parse(cookie, conn.url()) {
                    Ok(action) => log::trace!("cookie action: {:?}", action),
                    Err(e) => log::trace!("cookie parse error: {:?}", e),
                }
            }
        }

        Ok(())
    }
}
