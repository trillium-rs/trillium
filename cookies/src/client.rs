//! Client-side cookie middleware for [`trillium-client`][trillium_client].
//!
//! This module is gated behind the `client` feature flag. It provides [`Cookies`], a
//! [`ClientHandler`] that maintains an RFC 6265 cookie jar across requests issued by a single
//! [`Client`][trillium_client::Client]:
//!
//! - On each outbound request, it looks up cookies whose domain/path/secure attributes match the
//!   request URL and attaches them via the `Cookie` request header.
//! - On each response, it parses `Set-Cookie` headers and stores the resulting cookies in the jar
//!   with the response URL as the originating request URL (for domain/path attribution).
//!
//! Domain matching, public-suffix-list checks, path matching, expiration, and the `Secure` flag
//! are all handled by [`cookie_store::CookieStore`] internally. Seed the jar with
//! [`Cookies::with_store`] and read it for cloning or serialization with [`Cookies::borrow`].
//!
//! # Example
//!
//! ```
//! use trillium_client::Client;
//! use trillium_cookies::client::Cookies;
//! # use trillium_testing::client_config;
//!
//! let client = Client::new(client_config()).with_handler(Cookies::new());
//! ```

pub use cookie_store;
use cookie_store::CookieStore;
use std::sync::{Arc, RwLock};
use trillium_client::{
    ClientHandler, Conn,
    KnownHeaderName::{Cookie, SetCookie},
};

/// A [`ClientHandler`] that maintains an RFC 6265 cookie jar across requests.
///
/// See the [module-level documentation][self] for behavior details.
#[derive(Debug, Clone, Default)]
pub struct Cookies {
    store: Arc<RwLock<CookieStore>>,
}

impl Cookies {
    /// Construct a new [`Cookies`] handler with an empty jar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a new [`Cookies`] handler wrapping the provided [`CookieStore`].
    ///
    /// Use this to seed the jar from any prebuilt store — a deserialized snapshot, programmatic
    /// seeds, or any other source you can build a [`CookieStore`] from.
    pub fn with_store(store: CookieStore) -> Self {
        Self {
            store: Arc::new(RwLock::new(store)),
        }
    }

    /// Read-only access to the underlying [`CookieStore`] via a closure.
    ///
    /// Holds a read lock for the duration of the closure, so keep the work brief.
    ///
    /// Use this to clone a snapshot, inspect a specific cookie, or serialize the jar:
    ///
    /// ```rust
    /// # let cookies = trillium_cookies::client::Cookies::new();
    /// let snapshot = cookies.borrow(Clone::clone);
    /// ```
    ///
    /// To serialize, enable the `client-serialization` feature and use
    /// [`cookie_store::serde`]:
    ///
    /// ```rust
    /// use trillium_cookies::client::cookie_store::serde::save;
    /// let cookies = trillium_cookies::client::Cookies::new();
    /// let mut vec = vec![];
    /// cookies
    ///     .borrow(|store| save(store, &mut vec, serde_json::to_string))
    ///     .unwrap();
    /// ```
    pub fn borrow<T>(&self, borrow_fn: impl FnOnce(&CookieStore) -> T) -> T {
        let store = self.store.read().unwrap();
        borrow_fn(&store)
    }
}

impl ClientHandler for Cookies {
    async fn run(&self, conn: &mut Conn) -> trillium_client::Result<()> {
        let url = conn.url().clone();
        let header_value = {
            let store = self.store.read().unwrap();
            let pairs: Vec<String> = store
                .get_request_values(&url)
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            if pairs.is_empty() {
                None
            } else {
                Some(pairs.join("; "))
            }
        };
        if let Some(value) = header_value {
            conn.request_headers_mut().insert(Cookie, value);
        }
        Ok(())
    }

    async fn after_response(&self, conn: &mut Conn) -> trillium_client::Result<()> {
        let Some(values) = conn.response_headers().get_values(SetCookie) else {
            return Ok(());
        };
        let url = conn.url().clone();
        let mut store = self.store.write().unwrap();
        for value in values.iter().filter_map(|v| v.as_str()) {
            if let Err(e) = store.parse(value, &url) {
                log::debug!("ignoring malformed Set-Cookie {value:?}: {e}");
            }
        }
        Ok(())
    }
}
