use crate::Conn;
use trillium_http::{Body, Error, Headers, KnownHeaderName, Status, Version};

/// The extension trait handler authors use to drive the [`ClientHandler`] lifecycle.
///
/// [`ClientHandler`]: crate::ClientHandler
///
/// These methods govern flow within the handler chain — queue a follow-up request for the
/// [`IntoFuture for &mut Conn`][std::future::IntoFuture] trampoline to re-execute, or
/// stash / inspect / recover the transport-level error that runs through
/// `after_response`. They are meaningful only from inside a [`ClientHandler`]
/// implementation: external user code holding a [`Conn`] has no reason to call them. A
/// queued follow-up is picked up only by the trampoline the handler chain runs through;
/// an externally-installed error just turns into an `Err` on the next `.await`.
///
/// Bring the methods into scope with `use trillium_client::ConnExt;`. The split
/// from [`Conn`]'s inherent methods is intentional — these affordances live on a trait
/// so handler authors opt into them explicitly and user code holding a `Conn` directly
/// doesn't see them in IDE completion.
///
/// [`ClientHandler`]: crate::ClientHandler
pub trait ConnExt {
    /// Queue a follow-up [`Conn`] to be executed after the current cycle's
    /// `after_response` chain has fully unwound.
    ///
    /// The follow-up is picked up by the [`IntoFuture for &mut Conn`][std::future::IntoFuture]
    /// trampoline, which drains and recycles the current conn's response body, then runs
    /// a fresh `(run → network → after_response)` cycle on the follow-up. After the
    /// trampoline finishes, the user's conn handle holds the *terminal* response — the
    /// same shape they see today after a redirect chain.
    ///
    /// Setting a follow-up while one is already queued replaces the previous one
    /// (last-writer-wins). Handlers that want to be polite about not clobbering a
    /// follow-up queued by an earlier handler can peek via [`ConnExt::followup`]
    /// or take via [`ConnExt::take_followup`] first.
    ///
    /// An unrecovered error stash on the conn (see [`ConnExt::error`] and
    /// [`ConnExt::take_error`]) wins over a queued follow-up: when the current
    /// cycle ends with `Err`, the trampoline discards the queued follow-up and propagates
    /// the error. Recovery handlers that want the follow-up to run anyway (retry-on-error,
    /// stale-if-error cache) must call `take_error()` inside `after_response` before
    /// queuing.
    fn set_followup(&mut self, conn: Conn) -> &mut Self;

    /// Borrow the queued follow-up [`Conn`], if any, without consuming it.
    ///
    /// Returns `None` when no follow-up has been installed. Useful for "polite"
    /// composition — a handler that wants to avoid clobbering a follow-up queued by an
    /// earlier handler in the chain can check this before calling
    /// [`ConnExt::set_followup`].
    fn followup(&self) -> Option<&Conn>;

    /// Detach the queued follow-up [`Conn`], if any.
    ///
    /// Pairs with [`ConnExt::set_followup`] for handlers that want to revoke or
    /// inspect a follow-up queued by an earlier handler in the chain — e.g. take,
    /// mutate, and re-queue, or take and discard outright.
    fn take_followup(&mut self) -> Option<Conn>;

    /// Borrow the transport-level error stashed on this conn, if any.
    ///
    /// During a handler chain's `after_response` pass, this is `Some` when the network
    /// round-trip failed (connect refused, TLS handshake error, malformed HTTP frame,
    /// timeout, etc.). Observer handlers (logger, metrics) use this to record failures;
    /// recovery handlers (stale-if-error cache, retry-with-fallback) use it as the
    /// trigger to synthesize a fallback response and clear the error via
    /// [`ConnExt::take_error`].
    fn error(&self) -> Option<&Error>;

    /// Install a transport-level error on this conn.
    ///
    /// Mostly internal — the framework stashes round-trip errors here automatically so
    /// the handler chain's `after_response` runs and can recover. Handler-authored use
    /// is rare and usually means "synthesize a failure mode for a downstream recovery
    /// handler to observe."
    fn set_error(&mut self, error: Error) -> &mut Self;

    /// Take the transport-level error stashed on this conn, leaving `None` in its place.
    ///
    /// This is the recovery path: a handler that wants to convert a transport failure
    /// into a synthetic success response (stale-if-error cache, retry-with-fallback)
    /// calls this inside `after_response` to clear the stash before populating the
    /// response state synthetically. If no handler clears the error, it propagates as
    /// `Err` from the awaited conn.
    fn take_error(&mut self) -> Option<Error>;

    /// Mark this conn halted, skipping the network round-trip in the current cycle.
    ///
    /// Use this in combination with synthetic response state ([`Conn::set_status`],
    /// [`Conn::response_headers_mut`], [`ConnExt::set_response_body`]) when a handler
    /// wants to fully synthesize a response — cache hits, mocked responses, or
    /// circuit-breaker short-circuits. The halt flag is internal to the handler chain: the
    /// trampoline clears it on egress so the user's conn handle never observes residual
    /// halt state after the awaited conn returns.
    fn halt(&mut self) -> &mut Self;

    /// Set the halt flag explicitly.
    ///
    /// Same semantics as [`ConnExt::halt`] for the affirmative case. The explicit
    /// setter exists for the rare handler that wants to un-halt a conn another handler in
    /// the chain has halted.
    fn set_halted(&mut self, halted: bool) -> &mut Self;

    /// Whether this conn is halted within the current cycle.
    ///
    /// `after_response` handlers can use this to differentiate "synthetic response" from
    /// "transport-backed response" — e.g. a logger or metrics handler that wants to record
    /// cache hits distinctly from network-backed responses.
    fn is_halted(&self) -> bool;

    /// Install an override response body, replacing whatever transport-backed body would
    /// otherwise be read from the network.
    ///
    /// Used by handlers that synthesize responses — cache hits, mocked responses,
    /// stale-if-error fallbacks. Typically combined with [`Conn::set_status`],
    /// [`Conn::response_headers_mut`], and [`ConnExt::halt`] to construct a complete
    /// synthetic response.
    ///
    /// Accepts anything convertible to a [`Body`], so common patterns work directly:
    ///
    /// ```ignore
    /// conn.set_response_body("hello");
    /// conn.set_response_body(vec![1, 2, 3]);
    /// conn.set_response_body(Body::new_streaming(file_reader, Some(file_size)));
    /// ```
    ///
    /// Encoding for [`ResponseBody::read_string`] is determined by the response headers'
    /// Content-Type, just like a transport-backed body — set the appropriate header before
    /// or after this call as needed. The user-set `max_len` is enforced for override bodies
    /// as well as transport-backed ones.
    ///
    /// [`ResponseBody::read_string`]: crate::ResponseBody::read_string
    fn set_response_body(&mut self, body: impl Into<Body>) -> &mut Self;

    /// Owned chainable variant of [`ConnExt::set_response_body`].
    #[must_use]
    fn with_response_body(self, body: impl Into<Body>) -> Self
    where
        Self: Sized;

    /// Set the response status — handler-author synthesis.
    ///
    /// Setting a status on a conn that's about to be sent has no meaningful effect: the
    /// status reflects what the server returned. The only sensible uses are inside a
    /// handler synthesizing a response (cache hit, mocked response, stale-if-error
    /// fallback) — pair with [`ConnExt::set_response_body`],
    /// [`ConnExt::response_headers_mut`], and [`ConnExt::halt`].
    fn set_status(&mut self, status: Status) -> &mut Self;

    /// Owned chainable variant of [`ConnExt::set_status`].
    #[must_use]
    fn with_status(self, status: Status) -> Self
    where
        Self: Sized;

    /// Mutably borrow the response headers — handler-author synthesis.
    ///
    /// The read-only [`Conn::response_headers`] accessor stays inherent for user code that
    /// wants to inspect what the server returned. Mutating those headers only makes sense
    /// from inside a handler synthesizing a response.
    fn response_headers_mut(&mut self) -> &mut Headers;

    /// Replace the response headers wholesale — handler-author synthesis.
    fn set_response_headers(&mut self, response_headers: Headers) -> &mut Self;

    /// Mutably borrow the response trailers, if any — handler-author synthesis.
    fn response_trailers_mut(&mut self) -> Option<&mut Headers>;

    /// Install response trailers — handler-author synthesis.
    fn set_response_trailers(&mut self, response_trailers: Headers) -> &mut Self;
}

impl ConnExt for Conn {
    fn set_followup(&mut self, conn: Conn) -> &mut Self {
        self.followup = Some(Box::new(conn));
        self
    }

    fn followup(&self) -> Option<&Conn> {
        self.followup.as_deref()
    }

    fn take_followup(&mut self) -> Option<Conn> {
        self.followup.take().map(|b| *b)
    }

    fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    fn set_error(&mut self, error: Error) -> &mut Self {
        self.error = Some(error);
        self
    }

    fn take_error(&mut self) -> Option<Error> {
        self.error.take()
    }

    fn halt(&mut self) -> &mut Self {
        self.halted = true;
        self
    }

    fn set_halted(&mut self, halted: bool) -> &mut Self {
        self.halted = halted;
        self
    }

    fn is_halted(&self) -> bool {
        self.halted
    }

    fn set_response_body(&mut self, body: impl Into<Body>) -> &mut Self {
        let body: Body = body.into().without_chunked_framing();
        if let Some(len) = body.len() {
            self.response_headers_mut()
                .insert(KnownHeaderName::ContentLength, len.to_string())
                .remove(KnownHeaderName::TransferEncoding);
        } else {
            self.response_headers_mut()
                .remove(KnownHeaderName::ContentLength);
            if self.http_version == Version::Http1_1 {
                self.response_headers_mut()
                    .insert(KnownHeaderName::TransferEncoding, "chunked");
            }
        }
        // Recycle whatever body was here — once the override is installed, the transport
        // (if any) won't be read from again.
        drop(self.take_response_body());
        self.body_override = Some(body);
        self
    }

    fn with_response_body(mut self, body: impl Into<Body>) -> Self {
        self.set_response_body(body);
        self
    }

    fn set_status(&mut self, status: Status) -> &mut Self {
        self.status = Some(status);
        self
    }

    fn with_status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }

    fn response_headers_mut(&mut self) -> &mut Headers {
        &mut self.response_headers
    }

    fn set_response_headers(&mut self, response_headers: Headers) -> &mut Self {
        self.response_headers = response_headers;
        self
    }

    fn response_trailers_mut(&mut self) -> Option<&mut Headers> {
        self.response_trailers.as_mut()
    }

    fn set_response_trailers(&mut self, response_trailers: Headers) -> &mut Self {
        self.response_trailers = Some(response_trailers);
        self
    }
}
