use super::Conn;
use std::{borrow::Cow, future::poll_fn, sync::Arc};
use trillium_http::{
    Error, KnownHeaderName, Method, ReceivedBodyState, Result, Version,
    h2::H2Connection,
    headers::hpack::{self, FieldSection, PseudoHeaders},
};
use trillium_server_common::Transport;

impl Conn {
    /// Attempt to execute this request over a pooled HTTP/2 connection.
    ///
    /// Returns `Ok(true)` if a live pooled connection was found and the request was sent on it.
    /// Returns `Ok(false)` if no pooled h2 connection is available, signalling the caller to
    /// fall through to the h1 / fresh-connect path.
    pub(super) async fn try_exec_h2_pooled(&mut self) -> Result<bool> {
        let Some(h2_pool) = &self.h2_pool else {
            return Ok(false);
        };
        let origin = self.url.origin();
        let Some(h2) = h2_pool.peek_candidate(&origin) else {
            return Ok(false);
        };
        if !h2.swansong().state().is_running() {
            return Ok(false);
        }
        self.exec_h2_on_connection(h2).await?;
        Ok(true)
    }

    /// Promote a freshly-connected transport whose ALPN negotiated `h2` into an h2 connection,
    /// install it in the pool, and execute the request on a fresh stream.
    pub(super) async fn try_exec_h2_with_transport(
        &mut self,
        transport: Box<dyn Transport>,
    ) -> Result<()> {
        let h2 = H2Connection::new(self.context.clone());
        let initiator = h2.clone().run_client(transport);
        self.config.runtime().spawn(async move {
            if let Err(e) = initiator.await {
                log::debug!("h2 client connection terminated: {e}");
            }
        });

        if let Some(h2_pool) = &self.h2_pool {
            h2_pool.insert(
                self.url.origin(),
                crate::pool::PoolEntry::new(h2.clone(), None),
            );
        }

        self.exec_h2_on_connection(h2).await
    }

    async fn exec_h2_on_connection(&mut self, h2: Arc<H2Connection>) -> Result<()> {
        self.http_version = Version::Http2;
        self.headers_finalized = false;
        self.finalize_headers_h2()?;

        let pseudo_headers = self.build_pseudo_headers()?;
        let field_section = FieldSection::new(pseudo_headers, &self.request_headers);
        log::trace!("sending h2 headers:\n{field_section}");
        let mut encoded = Vec::with_capacity(self.context.config().request_buffer_initial_len());
        hpack::encode(&field_section, &mut encoded);

        let body = self.request_body.take();
        let (stream_id, submit, transport) = h2.open_stream(encoded, body).ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "h2 connection is shutting down",
            ))
        })?;
        log::trace!("h2 client opened stream {stream_id}");

        self.h2_connection = Some((h2.clone(), stream_id));
        self.transport = Some(Box::new(transport));

        // Sequential send-then-recv is fine for the C1 happy path: the driver pumps both
        // directions in parallel regardless of the order we await. Streaming-body cases that
        // benefit from recv-while-still-sending will arrive with C3.
        submit.await.map_err(Error::Io)?;
        self.recv_h2_response_headers(&h2, stream_id).await?;
        Ok(())
    }

    fn build_pseudo_headers(&self) -> Result<PseudoHeaders<'_>> {
        let mut pseudo = PseudoHeaders::default()
            .with_method(self.method)
            .with_authority(
                self.authority
                    .as_deref()
                    .ok_or(Error::UnexpectedUriFormat)?,
            );

        if self.method != Method::Connect {
            pseudo
                .set_path(Some(
                    self.path.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ))
                .set_scheme(Some(
                    self.scheme.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ));
        }

        Ok(pseudo)
    }

    pub(super) fn finalize_headers_h2(&mut self) -> Result<()> {
        if self.headers_finalized {
            return Ok(());
        }

        // :authority resolves the same way h3 does — Host header takes precedence (proxy /
        // virtual hosting), URL is the fallback.
        let authority = self
            .request_headers
            .remove(KnownHeaderName::Host)
            .and_then(|h| h.first().map(|v| Cow::Owned(v.to_string())))
            .or_else(|| {
                let host = self.url.host_str()?;
                Some(Cow::Owned(self.url.port().map_or_else(
                    || host.to_string(),
                    |port| format!("{host}:{port}"),
                )))
            })
            .ok_or(Error::UnexpectedUriFormat)?;

        self.authority = Some(authority);

        if let Some(target) = &self.request_target
            && self.method == Method::Options
        {
            self.scheme = Some(Cow::Owned(self.url.scheme().to_string()));
            self.path = Some(target.clone());
        } else if self.method == Method::Connect {
            self.scheme = None;
            self.path = None;
        } else {
            self.scheme = Some(Cow::Owned(self.url.scheme().to_string()));
            self.path = Some(Cow::Owned({
                let mut path = self.url.path().to_string();
                if let Some(query) = self.url.query() {
                    path.push('?');
                    path.push_str(query);
                }
                path
            }));
        }

        if let Some(len) = self.body_len()
            && len > 0
        {
            self.request_headers
                .insert(KnownHeaderName::ContentLength, len);
        }

        // RFC 9113 §8.2.2: connection-specific headers MUST NOT appear, and Expect:100-continue
        // is an h1-only mechanism. Any of these may have been added by a prior
        // `finalize_headers_h1` call before we diverted to h2 via ALPN.
        self.request_headers.remove_all([
            KnownHeaderName::Connection,
            KnownHeaderName::TransferEncoding,
            KnownHeaderName::KeepAlive,
            KnownHeaderName::ProxyConnection,
            KnownHeaderName::Upgrade,
            KnownHeaderName::Expect,
        ]);

        self.headers_finalized = true;
        Ok(())
    }

    async fn recv_h2_response_headers(
        &mut self,
        h2: &Arc<H2Connection>,
        stream_id: u32,
    ) -> Result<()> {
        let field_section = poll_fn(|cx| h2.poll_response_headers(stream_id, cx))
            .await
            .map_err(Error::Io)?;
        log::trace!("received h2 response:\n{field_section}");

        self.status = field_section.pseudo_headers().status();
        self.response_headers = field_section.into_headers().into_owned();
        self.response_body_state = ReceivedBodyState::new_h2();

        Ok(())
    }
}
