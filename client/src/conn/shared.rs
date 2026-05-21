use super::{Body, Conn, Transport, TypeSet};
use crate::{ClientHandler, ConnExt, Error, Result, Version};
use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    future::{Future, IntoFuture},
    mem,
    pin::Pin,
};
use trillium_http::{ProtocolSession, Upgrade};

/// A wrapper error for [`trillium_http::Error`] or, depending on json serializer feature, either
/// `sonic_rs::Error` or `serde_json::Error`. Only available when either the `sonic-rs` or
/// `serde_json` cargo features are enabled.
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
#[derive(thiserror::Error, Debug)]
pub enum ClientSerdeError {
    /// A [`trillium_http::Error`]
    #[error(transparent)]
    HttpError(#[from] Error),

    #[cfg(feature = "sonic-rs")]
    /// A [`sonic_rs::Error`]
    #[error(transparent)]
    JsonError(#[from] sonic_rs::Error),

    #[cfg(feature = "serde_json")]
    /// A [`serde_json::Error`]
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),
}

impl Conn {
    pub(crate) async fn exec(&mut self) -> Result<()> {
        // Arc-clone to dodge conflict with the `&mut self` we pass to `run`.
        let handler = self.client.handler().clone();
        handler.run(self).await?;

        if !self.halted {
            // Stash, don't return: `after_response` runs unconditionally so recovery handlers
            // (stale-if-error, retry-with-fallback) get a chance to clear it.
            if let Err(e) = self.exec_network().await {
                self.error = Some(e);
            }
        } else {
            log::trace!("conn is halted, skipping network round-trip");
        }

        // Reverse order, regardless of halt/error — mirrors server-side `before_send`.
        handler.after_response(self).await?;

        if let Some(e) = self.error.take() {
            Err(e)
        } else {
            Ok(())
        }
    }

    async fn exec_network(&mut self) -> Result<()> {
        if matches!(self.http_version, Version::Http0_9) {
            return Err(Error::UnsupportedVersion(self.http_version));
        }

        if self.try_exec_h3().await? {
            return Ok(());
        }
        if self.try_exec_h2_pooled().await? {
            return Ok(());
        }

        // Prior-knowledge h2: caller asserted h2, skip h1/ALPN. Useful for TLS connectors
        // that don't expose `negotiated_alpn` (e.g. native-tls). No fallback — a non-h2
        // server here surfaces as a plain IO error.
        if self.http_version == Version::Http2 {
            return self.exec_h2_prior_knowledge().await;
        }

        self.exec_h1_or_promote_h2().await
    }

    pub(crate) fn body_len(&self) -> Option<u64> {
        if let Some(ref body) = self.request_body {
            body.len()
        } else {
            Some(0)
        }
    }

    pub(crate) fn finalize_headers(&mut self) -> Result<()> {
        match self.http_version {
            Version::Http1_0 | Version::Http1_1 => self.finalize_headers_h1(),
            Version::Http2 => self.finalize_headers_h2(),
            Version::Http3 if self.client.h3().is_some() => self.finalize_headers_h3(),
            other => Err(Error::UnsupportedVersion(other)),
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        log::trace!("dropping client conn");
        drop(self.take_response_body());
    }
}

impl From<Conn> for Body {
    fn from(mut conn: Conn) -> Body {
        // body_override (e.g. cache hit, set via `set_response_body`) bypasses the transport;
        // transport pooling is left to `Drop`.
        if let Some(body) = conn.body_override.take() {
            return body;
        }

        match conn.take_received_body(true) {
            Some(rb) => rb.into(),
            None => Body::default(),
        }
    }
}

impl From<Conn> for Upgrade<Box<dyn Transport>> {
    /// Convert a client conn into a [`trillium_http::Upgrade`] after response headers
    /// arrive, handing off the open transport for direct `AsyncRead` / `AsyncWrite`
    /// exchange with per-protocol framing applied.
    ///
    /// # Panics
    ///
    /// Panics if the conn has no live transport (request not yet sent, or transport
    /// already taken).
    fn from(mut conn: Conn) -> Self {
        // `Conn: Drop` rules out destructuring — pull each field with `mem::take` /
        // `mem::replace`. New fields on `Conn` won't show up here automatically.
        let path = conn.path.take().unwrap_or_else(|| match conn.url.query() {
            Some(q) => Cow::Owned(format!("{}?{q}", conn.url.path())),
            None => Cow::Owned(conn.url.path().to_owned()),
        });
        let secure = conn.url.scheme() == "https";

        Upgrade::from_parts(
            mem::take(&mut conn.response_headers),
            mem::take(&mut conn.request_headers),
            path,
            conn.method,
            conn.transport
                .take()
                .expect("client conn has no transport — request not yet sent"),
            mem::take(&mut conn.buffer),
            mem::take(&mut conn.state),
            conn.context.clone(),
            None,
            conn.authority.take(),
            conn.scheme.take(),
            mem::replace(&mut conn.protocol_session, ProtocolSession::Http1),
            conn.protocol.take(),
            conn.http_version,
            conn.status,
            secure,
            // Client-side inbound = response body.
            mem::take(&mut conn.response_body_state),
            // Carry through any pre-upgrade-decoded trailers so a downstream reader
            // can observe them.
            conn.response_trailers.take(),
        )
    }
}

impl IntoFuture for Conn {
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;
    type Output = Result<Conn>;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move { (&mut self).await.map(|()| self) })
    }
}

impl<'conn> IntoFuture for &'conn mut Conn {
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'conn>>;
    type Output = Result<()>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            // Re-issuing handlers (FollowRedirects, retry, auth-refresh) queue a follow-up
            // via `set_followup` in `after_response`; we recycle, swap, re-exec.
            loop {
                let result = if let Some(duration) = self.timeout {
                    self.client
                        .connector()
                        .runtime()
                        .timeout(duration, self.exec())
                        .await
                        .unwrap_or(Err(Error::TimedOut("Conn", duration)))
                } else {
                    self.exec().await
                };

                // `halted` is handler-internal; don't leak it out to the caller.
                self.halted = false;

                if let Err(e) = result {
                    // Unrecovered error wins over any queued follow-up. Recovery handlers
                    // that want the follow-up to run must `take_error()` in `after_response`.
                    self.followup = None;
                    return Err(e);
                }

                let Some(next) = self.take_followup() else {
                    break;
                };

                if let Some(body) = self.take_response_body() {
                    body.recycle().await;
                }

                let _displaced = mem::replace(self, next);
            }
            Ok(())
        })
    }
}

impl Debug for Conn {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("authority", &self.authority)
            .field("buffer", &String::from_utf8_lossy(&self.buffer))
            .field("client", &self.client)
            .field("protocol_session", &self.protocol_session)
            .field("http_version", &self.http_version)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("request_body", &self.request_body)
            .field("request_headers", &self.request_headers)
            .field("request_target", &self.request_target)
            .field("request_trailers", &self.request_trailers)
            .field("response_body_state", &self.response_body_state)
            .field("response_headers", &self.response_headers)
            .field("response_trailers", &self.response_trailers)
            .field("scheme", &self.scheme)
            .field("state", &self.state)
            .field("status", &self.status)
            .field("url", &self.url)
            .finish()
    }
}

impl AsRef<TypeSet> for Conn {
    fn as_ref(&self) -> &TypeSet {
        &self.state
    }
}

impl AsMut<TypeSet> for Conn {
    fn as_mut(&mut self) -> &mut TypeSet {
        &mut self.state
    }
}
