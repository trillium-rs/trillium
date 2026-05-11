use super::{Body, Conn, ReceivedBody, ReceivedBodyState, Transport, TypeSet, encoding};
use crate::{ClientHandler, Error, Result, Version, pool::PoolEntry};
use futures_lite::{AsyncWriteExt, io};
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
        let Some(mut transport) = self.transport.take() else {
            log::trace!("no transport, nothing to do");

            return;
        };

        if !self.is_keep_alive() {
            log::trace!("not keep alive, closing");

            self.client
                .connector()
                .runtime()
                .clone()
                .spawn(async move { transport.close().await });

            return;
        }

        let Some(pool) = self.client.pool().cloned() else {
            return;
        };

        let origin = self.url.origin();

        if self.response_body_state == ReceivedBodyState::End {
            log::trace!(
                "response body has been read to completion, checking transport back into pool for \
                 {origin:?} ({:?})",
                transport.peer_addr()
            );
            pool.insert(origin, PoolEntry::new(transport, None));
        } else {
            let content_length = self.response_content_length();
            let buffer = mem::take(&mut self.buffer);
            let response_body_state = self.response_body_state;
            let encoding = encoding(&self.response_headers);
            self.client.connector().runtime().spawn(async move {
                let mut response_body = ReceivedBody::new(
                    content_length,
                    buffer,
                    transport,
                    response_body_state,
                    None,
                    encoding,
                );

                match io::copy(&mut response_body, io::sink()).await {
                    Ok(bytes) => {
                        let transport = response_body.take_transport().unwrap();
                        log::trace!("read {bytes} bytes in order to recycle conn",);
                        pool.insert(origin, PoolEntry::new(transport, None));
                    }

                    Err(ioerror) => log::error!("unable to recycle conn due to {}", ioerror),
                };
            });
        }
    }
}

impl From<Conn> for Body {
    fn from(conn: Conn) -> Body {
        let received_body: ReceivedBody<'static, _> = conn.into();
        received_body.into()
    }
}

impl From<Conn> for ReceivedBody<'static, Box<dyn Transport>> {
    fn from(mut conn: Conn) -> Self {
        let _ = conn.finalize_headers();
        let runtime = conn.client.connector().runtime();
        let origin = conn.url.origin();

        let on_completion = if conn.is_keep_alive()
            && let Some(pool) = conn.client.pool().cloned()
        {
            Box::new(move |transport: Box<dyn Transport>| {
                log::trace!("body transferred, returning to pool");
                pool.insert(origin.clone(), PoolEntry::new(transport, None));
            }) as Box<dyn FnOnce(Box<dyn Transport>) + Send + Sync + 'static>
        } else {
            Box::new(move |mut transport: Box<dyn Transport>| {
                runtime.spawn(async move { transport.close().await });
            }) as Box<dyn FnOnce(Box<dyn Transport>) + Send + Sync + 'static>
        };

        ReceivedBody::new(
            conn.response_content_length(),
            mem::take(&mut conn.buffer),
            conn.transport.take().unwrap(),
            conn.response_body_state,
            Some(on_completion),
            conn.response_encoding(),
        )
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
            if let Some(duration) = self.timeout {
                self.client
                    .connector()
                    .runtime()
                    .timeout(duration, self.exec())
                    .await
                    .unwrap_or(Err(Error::TimedOut("Conn", duration)))?;
            } else {
                self.exec().await?;
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
