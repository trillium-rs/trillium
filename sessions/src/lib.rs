pub use async_session::{CookieStore, MemoryStore, Session};

use async_session::{
    base64,
    hmac::{Hmac, Mac, NewMac},
    serde::Serialize,
    sha2::Sha256,
    SessionStore,
};

use myco::{async_trait, Conn, Grain};
use myco_cookies::{Cookie, CookiesConnExt, Key, SameSite};
use std::time::{Duration, SystemTime};

const BASE64_DIGEST_LEN: usize = 44;

pub struct Sessions<Store> {
    store: Store,
    cookie_path: String,
    cookie_name: String,
    cookie_domain: Option<String>,
    session_ttl: Option<Duration>,
    save_unchanged: bool,
    same_site_policy: SameSite,
    key: Key,
}

pub trait SessionConnExt {
    fn session(&self) -> &Session;
    fn with_session(self, key: &str, value: impl Serialize) -> Self;
    fn session_mut(&mut self) -> &mut Session;
}

impl SessionConnExt for Conn {
    fn session(&self) -> &Session {
        self.state()
            .expect("Sessions grain must be executed before calling SessionsExt::sessions")
    }

    fn with_session(mut self, key: &str, value: impl Serialize) -> Self {
        self.session_mut().insert(key, value).ok();
        self
    }

    fn session_mut(&mut self) -> &mut Session {
        self.state_mut()
            .expect("Sessions grain must be executed before calling SessionsExt::sessions_mut")
    }
}

#[async_trait]
impl<Store: SessionStore> Grain for Sessions<Store> {
    async fn run(&self, conn: Conn) -> Conn {
        let cookie_value = conn
            .cookies()
            .get(&self.cookie_name)
            .and_then(|cookie| self.verify_signature(cookie.value()).ok());

        let mut session = self.load_or_create(cookie_value).await;
        log::debug!("session: {:?}", session);

        if let Some(ttl) = self.session_ttl {
            session.expire_in(ttl);
        }

        conn.with_state(session)
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(session) = conn.take_state::<Session>() {
            let secure = conn.secure();
            if session.is_destroyed() {
                self.store.destroy_session(session).await.ok();
                conn.cookies_mut()
                    .remove(Cookie::named(self.cookie_name.clone()));
            } else if self.save_unchanged || session.data_changed() {
                if let Ok(Some(cookie_value)) = self.store.store_session(session).await {
                    conn.cookies_mut()
                        .add(self.build_cookie(secure, cookie_value));
                }
            }
        }

        conn
    }
}

impl<Store: SessionStore> Sessions<Store> {
    pub fn new(store: Store, secret: &[u8]) -> Self {
        Self {
            store,
            save_unchanged: true,
            cookie_path: "/".into(),
            cookie_name: "myco.sid".into(),
            cookie_domain: None,
            same_site_policy: SameSite::Strict,
            session_ttl: Some(Duration::from_secs(24 * 60 * 60)),
            key: Key::derive_from(secret),
        }
    }

    /// Sets a cookie path for this session grain.
    /// The default for this value is "/"
    pub fn with_cookie_path(mut self, cookie_path: impl AsRef<str>) -> Self {
        self.cookie_path = cookie_path.as_ref().to_owned();
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
    /// If you are running multiple myco applications on the same
    /// domain, you will need different values for each
    /// application. The default value is "myco.sid"
    pub fn with_cookie_name(mut self, cookie_name: impl AsRef<str>) -> Self {
        self.cookie_name = cookie_name.as_ref().to_owned();
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

    //--- methods below here are private ---

    async fn load_or_create(&self, cookie_value: Option<String>) -> Session {
        let session = match cookie_value {
            Some(cookie_value) => self.store.load_session(cookie_value).await.ok().flatten(),
            None => None,
        };

        session
            .and_then(|session| session.validate())
            .unwrap_or_default()
    }

    fn build_cookie(&self, secure: bool, cookie_value: String) -> Cookie<'static> {
        let mut cookie = Cookie::build(self.cookie_name.clone(), cookie_value)
            .http_only(true)
            .same_site(self.same_site_policy)
            .secure(secure)
            .path(self.cookie_path.clone())
            .finish();

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
    // https://github.com/SergioBenitez/cookie-rs/blob/master/src/secure/signed.rs#L33-L43
    /// Signs the cookie's value providing integrity and authenticity.
    fn sign_cookie(&self, cookie: &mut Cookie<'_>) {
        // Compute HMAC-SHA256 of the cookie's value.
        let mut mac = Hmac::<Sha256>::new_varkey(&self.key.signing()).expect("good key");
        mac.update(cookie.value().as_bytes());

        // Cookie's new value is [MAC | original-value].
        let mut new_value = base64::encode(&mac.finalize().into_bytes());
        new_value.push_str(cookie.value());
        cookie.set_value(new_value);
    }

    // the following is reused verbatim from
    // https://github.com/SergioBenitez/cookie-rs/blob/master/src/secure/signed.rs#L45-L63
    /// Given a signed value `str` where the signature is prepended to `value`,
    /// verifies the signed value and returns it. If there's a problem, returns
    /// an `Err` with a string describing the issue.
    fn verify_signature(&self, cookie_value: &str) -> Result<String, &'static str> {
        if cookie_value.len() < BASE64_DIGEST_LEN {
            return Err("length of value is <= BASE64_DIGEST_LEN");
        }

        // Split [MAC | original-value] into its two parts.
        let (digest_str, value) = cookie_value.split_at(BASE64_DIGEST_LEN);
        let digest = base64::decode(digest_str).map_err(|_| "bad base64 digest")?;

        // Perform the verification.
        let mut mac = Hmac::<Sha256>::new_varkey(&self.key.signing()).expect("good key");
        mac.update(value.as_bytes());
        mac.verify(&digest)
            .map(|_| value.to_string())
            .map_err(|_| "value did not verify")
    }
}
