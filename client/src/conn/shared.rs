use super::{Body, Conn, ReceivedBody, ReceivedBodyState, Transport, TypeSet, encoding};
use crate::{Error, Result, Version, pool::PoolEntry};
use futures_lite::{AsyncWriteExt, io};
use std::{
    fmt::{self, Debug, Formatter},
    future::{Future, IntoFuture},
    mem,
    pin::Pin,
};
use trillium_http::Upgrade;

/// A wrapper error for [`trillium_http::Error`] or, depending on json serializer feature, either
/// [`sonic_rs::Error`] or [`serde_json::Error`]. Only available when either the `sonic-rs` or
/// `serde_json` cargo features are enabled.
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
#[derive(thiserror::Error, Debug)]
pub enum ClientSerdeError {
    /// A [`trillium_http::Error`]
    #[error(transparent)]
    HttpError(#[from] Error),

    #[cfg(feature = "sonic-rs")]
    /// A [`serde_json::Error`]
    #[error(transparent)]
    JsonError(#[from] sonic_rs::Error),

    #[cfg(feature = "serde_json")]
    /// A [`serde_json::Error`]
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),
}

impl Conn {
    pub(crate) async fn exec(&mut self) -> Result<()> {
        if let Some(h3) = self.h3.clone()
            && self.try_exec_h3(&h3).await?
        {
            self.update_alt_svc_from_response(&h3);
            return Ok(());
        }

        self.exec_h1().await
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
            Version::Http0_9 | Version::Http1_0 | Version::Http1_1 => self.finalize_headers_h1(),
            Version::Http3 if self.h3.is_some() => self.finalize_headers_h3(),
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

            self.config
                .runtime()
                .clone()
                .spawn(async move { transport.close().await });

            return;
        }

        let Ok(Some(peer_addr)) = transport.peer_addr() else {
            return;
        };
        let Some(pool) = self.pool.take() else { return };

        let origin = self.url.origin();

        if self.response_body_state == ReceivedBodyState::End {
            log::trace!(
                "response body has been read to completion, checking transport back into pool for \
                 {}",
                &peer_addr
            );
            pool.insert(origin, PoolEntry::new(transport, None));
        } else {
            let content_length = self.response_content_length();
            let buffer = mem::take(&mut self.buffer);
            let response_body_state = self.response_body_state;
            let encoding = encoding(&self.response_headers);
            self.config.runtime().spawn(async move {
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
                        log::trace!(
                            "read {} bytes in order to recycle conn for {}",
                            bytes,
                            &peer_addr
                        );
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
        let runtime = conn.config.runtime();
        let origin = conn.url.origin();

        let on_completion = if conn.is_keep_alive()
            && let Some(pool) = conn.pool.take()
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
                self.config
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
            .field("url", &self.url)
            .field("method", &self.method)
            .field("request_headers", &self.request_headers)
            .field("response_headers", &self.response_headers)
            .field("status", &self.status)
            .field("request_body", &self.request_body)
            .field("pool", &self.pool)
            .field("h3", &self.h3.is_some())
            .field("buffer", &String::from_utf8_lossy(&self.buffer))
            .field("response_body_state", &self.response_body_state)
            .field("config", &self.config)
            .field("state", &self.state)
            .field("authority", &self.authority)
            .field("scheme", &self.scheme)
            .field("path", &self.path)
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
