//! Client-side follow-redirects middleware for [`trillium-client`][trillium_client].
//!
//! This module is gated behind the `client` feature flag. It provides [`FollowRedirects`], a
//! [`ClientHandler`] that automatically follows HTTP redirects (301, 302, 303, 307, 308) up to
//! a configurable limit, with sensible defaults around security-sensitive cases.
//!
//! # Behavior
//!
//! On a redirect response, [`FollowRedirects`] resolves the `Location` header against the
//! current request URL, applies the policy below, and re-issues the request through the same
//! client, so the connector and connection pool are reused.
//!
//! ## Method handling
//!
//! The redirect status determines whether the method changes and whether the request body is
//! replayed:
//!
//! | Status | Method change | Body |
//! |--------|---------------|------|
//! | 301 Moved Permanently | POST → GET, otherwise unchanged | dropped if method changed |
//! | 302 Found | POST → GET, otherwise unchanged | dropped if method changed |
//! | 303 See Other | always GET | always dropped |
//! | 307 Temporary Redirect | unchanged | replayed if static, dropped if streaming |
//! | 308 Permanent Redirect | unchanged | replayed if static, dropped if streaming |
//!
//! ## Body replay
//!
//! Static bodies (constructed via [`Body::new_static`] or any of the `From` conversions for
//! `Vec<u8>`, `&'static [u8]`, `String`, `&'static str`, etc.) are cloned and replayed on
//! redirect.
//!
//! Streaming bodies (constructed via [`Body::new_streaming`]) are one-shot. Once consumed by
//! the original request they cannot be replayed, and the redirected request is sent without
//! a body.
//!
//! [`Body::new_static`]: trillium_client::Body::new_static
//! [`Body::new_streaming`]: trillium_client::Body::new_streaming
//!
//! ## Cross-origin header filtering
//!
//! When the redirect target's origin (scheme + host + port) differs from the original, the
//! following headers are dropped from the redirected request to avoid credential leakage:
//!
//! - `Authorization`
//! - `Cookie`
//! - `Proxy-Authorization`
//!
//! ## Defaults
//!
//! - **Max redirects**: 10. Override with [`FollowRedirects::with_max_redirects`].
//! - **HTTPS → HTTP downgrade**: blocked. Allow with [`FollowRedirects::with_allow_downgrade`].
//! - **Cross-origin redirects**: allowed. Restrict with [`FollowRedirects::with_allowed_origins`].
//!
//! # Example
//!
//! ```no_run
//! use trillium_client::Client;
//! use trillium_redirect::client::FollowRedirects;
//! use trillium_testing::client_config;
//!
//! let client =
//!     Client::new(client_config()).with_handler(FollowRedirects::new().with_max_redirects(5));
//! ```

use std::{collections::HashSet, sync::Arc};
use trillium_client::{
    Body, ClientHandler, Conn, ConnExt,
    KnownHeaderName::{
        Authorization, Connection, ContentEncoding, ContentLength, ContentType, Cookie, Expect,
        Host, Location, ProxyAuthorization, TransferEncoding,
    },
    Method, Result, Status, Url,
    url::Origin,
};

/// A [`ClientHandler`] that automatically follows HTTP redirects.
///
/// See the [module-level documentation][self] for behavior and configuration.
#[derive(Debug, Clone)]
pub struct FollowRedirects {
    max_redirects: u32,
    allow_downgrade: bool,
    allowed_origins: Option<Arc<HashSet<Origin>>>,
}

impl Default for FollowRedirects {
    fn default() -> Self {
        Self::new()
    }
}

impl FollowRedirects {
    /// Construct a new [`FollowRedirects`] with default settings: max 10 redirects,
    /// HTTPS-to-HTTP downgrade blocked, all origins allowed.
    pub fn new() -> Self {
        Self {
            max_redirects: 10,
            allow_downgrade: false,
            allowed_origins: None,
        }
    }

    /// Set the maximum number of redirects to follow before erroring with
    /// [`RedirectError::TooMany`].
    #[must_use]
    pub fn with_max_redirects(mut self, max: u32) -> Self {
        self.max_redirects = max;
        self
    }

    /// Allow or block redirects from `https://` to `http://`. Default is blocked.
    #[must_use]
    pub fn with_allow_downgrade(mut self, allow: bool) -> Self {
        self.allow_downgrade = allow;
        self
    }

    /// Restrict redirects to the given allowlist of origins. Each `Url`'s
    /// [origin][trillium_client::url::Url::origin] (scheme + host + port) is added to the
    /// allowlist; the path/query/fragment of the input URLs is ignored.
    ///
    /// When set, redirects to any other origin error with [`RedirectError::OriginNotAllowed`].
    /// When unset (the default), all origins are permitted.
    #[must_use]
    pub fn with_allowed_origins<I: IntoIterator<Item = Url>>(mut self, origins: I) -> Self {
        let set: HashSet<Origin> = origins.into_iter().map(|u| u.origin()).collect();
        self.allowed_origins = Some(Arc::new(set));
        self
    }

    fn is_origin_allowed(&self, url: &Url) -> bool {
        match &self.allowed_origins {
            Some(allowed) => allowed.contains(&url.origin()),
            None => true,
        }
    }
}

/// Per-conn redirect counter, stashed in conn state by [`FollowRedirects`].
#[derive(Clone, Copy, Debug)]
struct RedirectCount(u32);

/// Snapshot of a replayable request body, stashed in conn state by [`FollowRedirects::run`]
/// so that [`FollowRedirects::after_response`] can rebuild the body for the redirected
/// request after the original was consumed by the network call.
#[derive(Debug)]
struct SavedBody(Body);

/// Errors produced by [`FollowRedirects`] when a redirect cannot be followed.
#[derive(thiserror::Error, Debug)]
pub enum RedirectError {
    /// The redirect chain exceeded the configured maximum.
    #[error("redirect chain exceeded {0} redirects")]
    TooMany(u32),

    /// The redirect target's origin is not in the configured allowlist.
    #[error("redirect to {0} not in allowed-origins list")]
    OriginNotAllowed(String),

    /// The original request was HTTPS and the redirect target is HTTP, but downgrade is not
    /// allowed.
    #[error("redirect from https to http blocked (call with_allow_downgrade(true) to permit)")]
    DowngradeBlocked,

    /// The redirect response had no `Location` header.
    #[error("3xx redirect response had no Location header")]
    MissingLocation,

    /// The `Location` header could not be parsed as a valid URL relative to the request URL.
    #[error("invalid Location header {value:?}: {error}")]
    InvalidLocation {
        /// The raw `Location` header value.
        value: String,
        /// The underlying URL parse error message.
        error: String,
    },
}

impl From<RedirectError> for trillium_client::Error {
    fn from(err: RedirectError) -> Self {
        trillium_client::Error::other(err)
    }
}

impl ClientHandler for FollowRedirects {
    async fn run(&self, conn: &mut Conn) -> Result<()> {
        // Snapshot replayable bodies into conn state so we can replay across a redirect.
        // Streaming bodies return None and are left alone — they're one-shot.
        let snapshot = conn.request_body().and_then(Body::try_clone);
        if let Some(snapshot) = snapshot {
            conn.insert_state(SavedBody(snapshot));
        }
        Ok(())
    }

    async fn after_response(&self, conn: &mut Conn) -> Result<()> {
        let Some(status) = conn.status() else {
            return Ok(());
        };
        let Some(redirect_kind) = classify_redirect(status) else {
            return Ok(());
        };

        // Resolve Location relative to the current request URL.
        let location = conn
            .response_headers()
            .get_str(Location)
            .ok_or(RedirectError::MissingLocation)?
            .to_string();
        let new_url = conn
            .url()
            .join(&location)
            .map_err(|e| RedirectError::InvalidLocation {
                value: location.clone(),
                error: e.to_string(),
            })?;

        // Apply policy.
        if !self.allow_downgrade && conn.url().scheme() == "https" && new_url.scheme() == "http" {
            return Err(RedirectError::DowngradeBlocked.into());
        }
        if !self.is_origin_allowed(&new_url) {
            return Err(RedirectError::OriginNotAllowed(new_url.to_string()).into());
        }

        // Bump count, error if over limit.
        let count = conn.state::<RedirectCount>().map_or(0, |c| c.0);
        if count >= self.max_redirects {
            return Err(RedirectError::TooMany(self.max_redirects).into());
        }

        // Decide method + whether to keep body.
        let original_method = conn.method();
        let (new_method, keep_body) = match redirect_kind {
            RedirectKind::SeeOther => (Method::Get, false),
            RedirectKind::PreserveMethod => (original_method, true),
            RedirectKind::PostToGet => {
                if original_method == Method::Post {
                    (Method::Get, false)
                } else {
                    (original_method, true)
                }
            }
        };

        let same_origin = conn.url().origin() == new_url.origin();

        // Build a fresh sibling conn from the same client and queue it as a follow-up rather than
        // awaiting it inline: the `IntoFuture for &mut Conn` loop recycles this conn's response
        // body and re-executes the follow-up as a flat top-level cycle, so observer handlers
        // (logger, conn-id, metrics) see every hop instead of having later hops nested inside the
        // first's lifecycle.
        let mut followup = conn.client().build_conn(new_method, new_url);

        // Copy request headers with a few categories stripped:
        // - protocol/transport-managed headers — `finalize_headers` will re-derive them for the new
        //   conn's body and url
        // - body-description headers — only meaningful when a body is being sent
        // - cross-origin credential headers — must not leak across origin boundaries
        let mut new_headers = conn.request_headers().clone();
        new_headers.remove_all([Host, ContentLength, TransferEncoding, Expect, Connection]);
        if !keep_body {
            new_headers.remove_all([ContentType, ContentEncoding]);
        }
        if !same_origin {
            new_headers.remove_all([Authorization, Cookie, ProxyAuthorization]);
        }
        *followup.request_headers_mut() = new_headers;

        // Replay the body if the redirect kind preserves it. Static bodies were snapshotted
        // in `run`; streaming bodies were one-shot and aren't replayable.
        if keep_body
            && let Some(saved) = conn.state::<SavedBody>()
            && let Some(replayed) = saved.0.try_clone()
        {
            followup.set_request_body(replayed);
        }

        followup.insert_state(RedirectCount(count + 1));
        conn.set_followup(followup);
        Ok(())
    }

    fn name(&self) -> std::borrow::Cow<'static, str> {
        "FollowRedirects".into()
    }
}

#[derive(Clone, Copy, Debug)]
enum RedirectKind {
    /// 303: always GET, drop body.
    SeeOther,
    /// 307/308: preserve method + body.
    PreserveMethod,
    /// 301/302: preserve method except POST → GET.
    PostToGet,
}

fn classify_redirect(status: Status) -> Option<RedirectKind> {
    match status {
        Status::MovedPermanently | Status::Found => Some(RedirectKind::PostToGet),
        Status::SeeOther => Some(RedirectKind::SeeOther),
        Status::TemporaryRedirect | Status::PermanentRedirect => Some(RedirectKind::PreserveMethod),
        _ => None,
    }
}
