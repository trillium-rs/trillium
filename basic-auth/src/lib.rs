#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

//! Basic authentication for trillium.rs
//!
//! ```rust,no_run
//! use trillium_basic_auth::BasicAuth;
//! trillium_smol::run((
//!     BasicAuth::new("trillium", "7r1ll1um").with_realm("rust"),
//!     |conn: trillium::Conn| async move { conn.ok("authenticated") },
//! ));
//! ```
//!
//! Requests that do not carry acceptable credentials are halted with a `401 Unauthorized` and a
//! `WWW-Authenticate` challenge, so handlers placed after [`BasicAuth`] only run for
//! authenticated requests. The authenticated username is available downstream through
//! [`BasicAuthConnExt::basic_auth_username`].
//!
//! Credentials can be checked against a single configured username and password
//! ([`BasicAuth::new`]) or against a predicate of your own ([`BasicAuth::validate_fn`],
//! [`BasicAuth::validate_async_fn`]).
//!
//! Because HTTP Basic transmits the password in a reversible encoding on every request, it is
//! only as confidential as the transport underneath it. Use it over https.

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use base64::{
    Engine,
    engine::general_purpose::{STANDARD as BASE64, STANDARD_NO_PAD as BASE64_NO_PAD},
};
use sha2::{Digest, Sha256};
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
};
use subtle::ConstantTimeEq;
use trillium::{
    Conn, Handler,
    KnownHeaderName::{Authorization, WwwAuthenticate},
    Status,
};

const SCHEME: &str = "Basic ";

/// basic auth handler
#[derive(Debug)]
pub struct BasicAuth {
    validation: Validation,
    realm: Option<String>,
    www_authenticate: String,
}

enum Validation {
    /// sha256 of a single configured credential, compared in constant time
    Digest([u8; 32]),
    Predicate(PredicateFn),
    AsyncPredicate(AsyncPredicateFn),
}

impl Debug for Validation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Digest(_) => "Digest",
            Self::Predicate(_) => "Predicate",
            Self::AsyncPredicate(_) => "AsyncPredicate",
        };
        f.debug_tuple(name).field(&format_args!("..")).finish()
    }
}

struct PredicateFn(Box<dyn Fn(&Credentials) -> bool + Send + Sync + 'static>);

type BoxFuture = Pin<Box<dyn Future<Output = bool> + Send + 'static>>;
struct AsyncPredicateFn(Box<dyn Fn(Credentials) -> BoxFuture + Send + Sync + 'static>);

/// basic auth username-password credentials
#[derive(Clone, PartialEq, Eq, fieldwork::Fieldwork)]
#[fieldwork(get)]
pub struct Credentials {
    /// username
    username: String,

    /// password
    password: String,
}

impl Debug for Credentials {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"<<secret>>")
            .finish()
    }
}

impl Credentials {
    /// build credentials from a username and password
    ///
    /// A username that contains a colon can never be sent by a client, because HTTP Basic
    /// separates the two with the first colon.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Extract and decode the credentials from a conn's `Authorization` header, if it carries
    /// well-formed Basic credentials.
    ///
    /// This performs no validation whatsoever — it is the parsing half of this crate, for
    /// applications that need the password itself, such as the convention of sending an api key
    /// as the password with a placeholder username.
    pub fn from_conn(conn: &Conn) -> Option<Self> {
        Self::from_header(conn.request_headers().get_str(Authorization)?)
    }

    fn from_header(header: &str) -> Option<Self> {
        let token = header
            .get(..SCHEME.len())
            .filter(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
            .map(|scheme| &header[scheme.len()..])?;

        let decoded = BASE64
            .decode(token)
            .or_else(|_| BASE64_NO_PAD.decode(token))
            .ok()?;
        let decoded = String::from_utf8(decoded).ok()?;
        let (username, password) = decoded.split_once(':')?;
        Some(Self::new(username, password))
    }

    /// The digest is over a length-prefixed username so that no two distinct credentials share
    /// one, even though `:` cannot appear in a username received from a client.
    fn digest(&self) -> [u8; 32] {
        Sha256::new()
            .chain_update(
                u64::try_from(self.username.len())
                    .unwrap_or(u64::MAX)
                    .to_le_bytes(),
            )
            .chain_update(&self.username)
            .chain_update(&self.password)
            .finalize()
            .into()
    }
}

impl BasicAuth {
    /// build a new basic auth handler that accepts exactly this username and password
    ///
    /// Only a digest of the credentials is retained, and it is compared in constant time.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self::from_validation(Validation::Digest(
            Credentials::new(username, password).digest(),
        ))
    }

    /// build a new basic auth handler that accepts any credentials for which the provided
    /// predicate returns true
    ///
    /// ```
    /// # use trillium_basic_auth::BasicAuth;
    /// let basic_auth = BasicAuth::validate_fn(|credentials| {
    ///     credentials.username().starts_with("admin-")
    ///         && credentials.password() == std::env::var("ADMIN_PASSWORD").unwrap()
    /// });
    /// ```
    ///
    /// A predicate that compares secrets should do so in constant time, as [`BasicAuth::new`]
    /// does.
    pub fn validate_fn<F>(predicate: F) -> Self
    where
        F: Fn(&Credentials) -> bool + Send + Sync + 'static,
    {
        Self::from_validation(Validation::Predicate(PredicateFn(Box::new(predicate))))
    }

    /// build a new basic auth handler that accepts any credentials for which the provided async
    /// predicate returns true, such as a database lookup and a password hash comparison
    ///
    /// ```
    /// # use trillium_basic_auth::BasicAuth;
    /// # async fn look_up(username: &str) -> Option<String> { None }
    /// let basic_auth = BasicAuth::validate_async_fn(|credentials| async move {
    ///     match look_up(credentials.username()).await {
    ///         Some(hash) => verify(credentials.password(), &hash),
    ///         None => false,
    ///     }
    /// });
    /// # fn verify(password: &str, hash: &str) -> bool { false }
    /// ```
    pub fn validate_async_fn<F, Fut>(predicate: F) -> Self
    where
        F: Fn(Credentials) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = bool> + Send + 'static,
    {
        Self::from_validation(Validation::AsyncPredicate(AsyncPredicateFn(Box::new(
            move |credentials| Box::pin(predicate(credentials)),
        ))))
    }

    fn from_validation(validation: Validation) -> Self {
        Self {
            validation,
            realm: None,
            www_authenticate: String::from("Basic"),
        }
    }

    /// provide a realm for the www-authenticate response sent by this handler
    pub fn with_realm(mut self, realm: &str) -> Self {
        self.www_authenticate = format!("Basic realm=\"{}\"", realm.replace('\"', "\\\""));
        self.realm = Some(String::from(realm));
        self
    }

    /// the realm provided to [`BasicAuth::with_realm`], if any
    pub fn realm(&self) -> Option<&str> {
        self.realm.as_deref()
    }

    async fn is_allowed(&self, credentials: &Credentials) -> bool {
        match &self.validation {
            Validation::Digest(expected) => credentials.digest().ct_eq(expected).into(),
            Validation::Predicate(PredicateFn(predicate)) => predicate(credentials),
            Validation::AsyncPredicate(AsyncPredicateFn(predicate)) => {
                predicate(credentials.clone()).await
            }
        }
    }

    fn deny(&self, conn: Conn) -> Conn {
        conn.with_status(Status::Unauthorized)
            .with_response_header(WwwAuthenticate, self.www_authenticate.clone())
            .halt()
    }
}

struct AuthenticatedUsername(String);

/// extension trait for reading the authenticated username
pub trait BasicAuthConnExt {
    /// the username that [`BasicAuth`] accepted for this conn, if any
    fn basic_auth_username(&self) -> Option<&str>;
}

impl BasicAuthConnExt for Conn {
    fn basic_auth_username(&self) -> Option<&str> {
        self.state::<AuthenticatedUsername>()
            .map(|AuthenticatedUsername(username)| &**username)
    }
}

impl Handler for BasicAuth {
    async fn run(&self, conn: Conn) -> Conn {
        let Some(credentials) = Credentials::from_conn(&conn) else {
            return self.deny(conn);
        };

        if self.is_allowed(&credentials).await {
            conn.with_state(AuthenticatedUsername(credentials.username))
        } else {
            self.deny(conn)
        }
    }
}
