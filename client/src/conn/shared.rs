use super::{Body, Conn, Transport, TypeSet};
use crate::{ClientHandler, ConnExt, Error, Result, Version};
use std::{
    fmt::{self, Debug, Formatter},
    future::{Future, IntoFuture},
    mem,
    pin::Pin,
};
use trillium_http::Upgrade;

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
        // Handler.run runs before the network round-trip, in declared order. A handler may halt
        // (e.g. on a cache hit) to short-circuit the network entirely. We clone the Arc-shared
        // handler off the client to avoid conflicting with the mut borrow we pass to `run`.
        let handler = self.client.handler().clone();
        handler.run(self).await?;

        if !self.halted {
            // Stash transport errors on the conn so the handler chain's `after_response` runs
            // and can recover (e.g. stale-if-error cache, retry-with-fallback). If no handler
            // takes the error, it propagates from this fn at the end.
            if let Err(e) = self.exec_network().await {
                self.error = Some(e);
            }
        } else {
            log::trace!("conn is halted, skipping network round-trip");
        }

        // Handler.after_response runs after the network call (or after a halt-skipped network
        // call) in *reverse* order, regardless of halt status or transport error. This mirrors
        // server-side `before_send` semantics so that loggers and metrics handlers placed after
        // a cache see both cache hits and transport-backed responses, and recovery handlers
        // (stale-if-error, retry) get a chance to clear `conn.error`.
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

        // h2 prior knowledge: `http_version = Http2` is an assertion that the server speaks
        // h2, so we skip h1 entirely. Over `http://` this is h2c (cleartext immediate
        // preface); over `https://` it bypasses ALPN-readback and starts the h2 driver
        // directly after the TLS handshake — useful for TLS connectors that don't expose
        // `negotiated_alpn` (e.g. native-tls today). Either way, there's no fallback path:
        // a server that doesn't actually speak h2 surfaces as a plain IO error.
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
        // An override response body (installed by middleware via `set_response_body`, e.g. on
        // a cache hit) bypasses the transport entirely. The transport — if any is still
        // present — is left on the conn for `Drop` to pool or close as usual.
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
    fn from(mut conn: Conn) -> Self {
        Upgrade::new(
            mem::take(&mut conn.request_headers),
            conn.url.path().to_string(),
            conn.method,
            conn.transport.take().unwrap(),
            mem::take(&mut conn.buffer),
            conn.http_version(),
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
            // The trampoline: a re-issuing handler (FollowRedirects, retry, auth-refresh)
            // queues a follow-up via `conn.set_followup(...)` from its `after_response`.
            // We pick it up here, recycle the current response body so the next request can
            // reuse the pooled h1 transport synchronously, then swap the follow-up into place
            // and run another full cycle on it. The displaced conn drops at the end of the
            // iteration; we've already taken its body so Drop is a no-op for transport
            // recycling.
            //
            // Error precedence: an unrecovered error wins over a queued follow-up. If exec
            // returns Err, we discard any queued follow-up before propagating so the conn
            // doesn't carry a stale follow-up out to the caller (which would otherwise be
            // picked up on a subsequent `.await`). Recovery handlers that want the follow-up
            // to run must `take_error()` to clear the stash inside `after_response`.
            //
            // Egress hygiene: `halted` is handler-internal state — once exec finishes its
            // cycle, the user's conn handle should never observe residual halt. We clear it
            // on both the success and error return paths.
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

                self.halted = false;

                if let Err(e) = result {
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
