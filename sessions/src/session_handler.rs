const BASE64_DIGEST_LEN: usize = 44;
use async_session::{
    base64,
    hmac::{Hmac, Mac, NewMac},
    sha2::Sha256,
    Session, SessionStore,
};
use std::{
    fmt::{self, Debug, Formatter},
    iter,
    time::{Duration, SystemTime},
};
use trillium::{Conn, Handler};
use trillium_cookies::{
    cookie::{Cookie, Key, SameSite},
    CookiesConnExt,
};

/**
# Handler to enable sessions.

See crate-level docs for an overview of this crate's approach to
sessions and security.
*/

pub struct SessionHandler<Store> {
    store: Store,
    cookie_path: String,
    cookie_name: String,
    cookie_domain: Option<String>,
    session_ttl: Option<Duration>,
    save_unchanged: bool,
    same_site_policy: SameSite,
    key: Key,
    older_keys: Vec<Key>,
}

impl<Store: SessionStore> Debug for SessionHandler<Store> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionHandler")
            .field("store", &self.store)
            .field("cookie_path", &self.cookie_path)
            .field("cookie_name", &self.cookie_name)
            .field("cookie_domain", &self.cookie_domain)
            .field("session_ttl", &self.session_ttl)
            .field("save_unchanged", &self.save_unchanged)
            .field("same_site_policy", &self.same_site_policy)
            .field("key", &"<<secret>>")
            .field("older_keys", &"<<secret>>")
            .finish()
    }
}

impl<Store: SessionStore> SessionHandler<Store> {
    /**
    Constructs a SessionHandler from the given
    [`async_session::SessionStore`] and secret. The `secret` MUST be
    at least 32 bytes long, and MUST be cryptographically random to be
    secure. It is recommended to retrieve this at runtime from the
    environment instead of compiling it into your application.

    # Panics

    SessionHandler::new will panic if the secret is fewer than
    32 bytes.

    # Defaults

    The defaults for SessionHandler are:
    * cookie path: "/"
    * cookie name: "trillium.sid"
    * session ttl: one day
    * same site: strict
    * save unchanged: enabled
    * older secrets: none

    # Customization

    Although the above defaults are appropriate for most applications,
    they can be overridden. Please be careful changing these settings,
    as they can weaken your application's security:

    ```rust
    # use std::time::Duration;
    # use trillium_sessions::{SessionHandler, MemoryStore};
    # use trillium_cookies::{CookiesHandler, cookie::SameSite};
    # std::env::set_var("TRILLIUM_SESSION_SECRETS", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    // this logic will be unique to your deployment
    let secrets_var = std::env::var("TRILLIUM_SESSION_SECRETS").unwrap();
    let session_secrets = secrets_var.split(' ').collect::<Vec<_>>();

    let handler = (
        CookiesHandler::new(),
        SessionHandler::new(MemoryStore::new(), session_secrets[0])
            .with_cookie_name("custom.cookie.name")
            .with_cookie_path("/some/path")
            .with_cookie_domain("trillium.rs")
            .with_same_site_policy(SameSite::Strict)
            .with_session_ttl(Some(Duration::from_secs(1)))
            .with_older_secrets(&session_secrets[1..])
            .without_save_unchanged()
    );

    ```
    */
    pub fn new(store: Store, secret: impl AsRef<[u8]>) -> Self {
        Self {
            store,
            save_unchanged: true,
            cookie_path: "/".into(),
            cookie_name: "trillium.sid".into(),
            cookie_domain: None,
            same_site_policy: SameSite::Lax,
            session_ttl: Some(Duration::from_secs(24 * 60 * 60)),
            key: Key::derive_from(secret.as_ref()),
            older_keys: vec![],
        }
    }

    /// Sets a cookie path for this session handler.
    /// The default for this value is "/"
    pub fn with_cookie_path(mut self, cookie_path: impl AsRef<str>) -> Self {
        cookie_path.as_ref().clone_into(&mut self.cookie_path);
        self
    }

    /// Sets a session ttl. This will be used both for the cookie
    /// expiry and also for the session-internal expiry.
    ///
    /// The default for this value is one day. Set this to None to not
    /// set a cookie or session expiry. This is not recommended.
    pub fn with_session_ttl(mut self, session_ttl: Option<Duration>) -> Self {
        self.session_ttl = session_ttl;
        self
    }

    /// Sets the name of the cookie that the session is stored with or in.
    ///
    /// If you are running multiple trillium applications on the same
    /// domain, you will need different values for each
    /// application. The default value is "trillium.sid"
    pub fn with_cookie_name(mut self, cookie_name: impl AsRef<str>) -> Self {
        cookie_name.as_ref().clone_into(&mut self.cookie_name);
        self
    }

    /// Disables the `save_unchanged` setting. When `save_unchanged`
    /// is enabled, a session will cookie will always be set. With
    /// `save_unchanged` disabled, the session data must be modified
    /// from the `Default` value in order for it to save. If a session
    /// already exists and its data unmodified in the course of a
    /// request, the session will only be persisted if
    /// `save_unchanged` is enabled.
    pub fn without_save_unchanged(mut self) -> Self {
        self.save_unchanged = false;
        self
    }

    /// Sets the same site policy for the session cookie. Defaults to
    /// SameSite::Strict. See [incrementally better
    /// cookies](https://tools.ietf.org/html/draft-west-cookie-incrementalism-01)
    /// for more information about this setting
    pub fn with_same_site_policy(mut self, policy: SameSite) -> Self {
        self.same_site_policy = policy;
        self
    }

    /// Sets the domain of the cookie.
    pub fn with_cookie_domain(mut self, cookie_domain: impl AsRef<str>) -> Self {
        self.cookie_domain = Some(cookie_domain.as_ref().to_owned());
        self
    }

    /// Sets optional older signing keys that will not be used to sign
    /// cookies, but can be used to validate previously signed
    /// cookies.
    pub fn with_older_secrets(mut self, secrets: &[impl AsRef<[u8]>]) -> Self {
        self.older_keys = secrets
            .iter()
            .map(AsRef::as_ref)
            .map(Key::derive_from)
            .collect();
        self
    }

    //--- methods below here are private ---

    async fn load_or_create(&self, cookie_value: Option<&str>) -> Session {
        let session = match cookie_value {
            Some(cookie_value) => self
                .store
                .load_session(String::from(cookie_value))
                .await
                .ok()
                .flatten(),
            None => None,
        };

        session
            .and_then(|session| session.validate())
            .unwrap_or_default()
    }

    fn build_cookie(&self, secure: bool, cookie_value: String) -> Cookie<'static> {
        let mut cookie: Cookie<'static> = Cookie::build((self.cookie_name.clone(), cookie_value))
            .http_only(true)
            .same_site(self.same_site_policy)
            .secure(secure)
            .path(self.cookie_path.clone())
            .into();

        if let Some(ttl) = self.session_ttl {
            cookie.set_expires(Some((SystemTime::now() + ttl).into()));
        }

        if let Some(cookie_domain) = self.cookie_domain.clone() {
            cookie.set_domain(cookie_domain)
        }

        self.sign_cookie(&mut cookie);

        cookie
    }
    // the following is reused verbatim from
    // https://github.com/SergioBenitez/cookie-rs/blob/master/src/secure/signed.rs#L37-46
    /// Signs the cookie's value providing integrity and authenticity.
    fn sign_cookie(&self, cookie: &mut Cookie<'_>) {
        // Compute HMAC-SHA256 of the cookie's value.
        let mut mac = Hmac::<Sha256>::new_from_slice(self.key.signing()).expect("good key");
        mac.update(cookie.value().as_bytes());

        // Cookie's new value is [MAC | original-value].
        let mut new_value = base64::encode(mac.finalize().into_bytes());
        new_value.push_str(cookie.value());
        cookie.set_value(new_value);
    }

    // the following is reused verbatim from
    // https://github.com/SergioBenitez/cookie-rs/blob/master/src/secure/signed.rs#L51-L66
    /// Given a signed value `str` where the signature is prepended to `value`,
    /// verifies the signed value and returns it. If there's a problem, returns
    /// an `Err` with a string describing the issue.
    fn verify_signature<'a>(&self, cookie_value: &'a str) -> Option<&'a str> {
        if cookie_value.len() < BASE64_DIGEST_LEN {
            log::trace!("length of value is <= BASE64_DIGEST_LEN");
            return None;
        }

        // Split [MAC | original-value] into its two parts.
        let (digest_str, value) = cookie_value.split_at(BASE64_DIGEST_LEN);
        let digest = match base64::decode(digest_str) {
            Ok(digest) => digest,
            Err(_) => {
                log::trace!("bad base64 digest");
                return None;
            }
        };

        iter::once(&self.key)
            .chain(self.older_keys.iter())
            .find_map(|key| {
                let mut mac = Hmac::<Sha256>::new_from_slice(key.signing()).expect("good key");
                mac.update(value.as_bytes());
                mac.verify(&digest).ok()
            })
            .map(|_| value)
    }
}

impl<Store: SessionStore> Handler for SessionHandler<Store> {
    async fn run(&self, mut conn: Conn) -> Conn {
        let session = conn.take_state::<Session>();

        let cookie_value = conn
            .cookies()
            .get(&self.cookie_name)
            .and_then(|cookie| self.verify_signature(cookie.value()));

        let mut session = match session {
            Some(session) => session,
            None => self.load_or_create(cookie_value).await,
        };

        if let Some(ttl) = self.session_ttl {
            session.expire_in(ttl);
        }

        conn.with_state(session)
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(session) = conn.take_state::<Session>() {
            let session_to_keep = session.clone();
            let secure = conn.is_secure();
            if session.is_destroyed() {
                self.store.destroy_session(session).await.ok();
                conn.cookies_mut()
                    .remove(Cookie::from(self.cookie_name.clone()));
            } else if self.save_unchanged || session.data_changed() {
                match self.store.store_session(session).await {
                    Ok(Some(cookie_value)) => {
                        conn.cookies_mut()
                            .add(self.build_cookie(secure, cookie_value));
                    }

                    Ok(None) => {}

                    Err(e) => {
                        log::error!("could not store session:\n\n{e}")
                    }
                }
            }

            conn.with_state(session_to_keep)
        } else {
            conn
        }
    }
}

/// Alias for [`SessionHandler::new`]
pub fn sessions<Store>(store: Store, secret: impl AsRef<[u8]>) -> SessionHandler<Store>
where
    Store: SessionStore,
{
    SessionHandler::new(store, secret)
}
