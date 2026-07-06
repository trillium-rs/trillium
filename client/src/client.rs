use crate::{
    ClientHandler, Conn, IntoUrl, Pool, USER_AGENT, client_handler::ArcedClientHandler,
    conn::H2Pooled, h3::H3ClientState,
};
use std::{any::Any, fmt::Debug, sync::Arc, time::Duration};
use trillium_http::{
    HeaderName, HeaderValues, Headers, HttpContext, KnownHeaderName, Method, ProtocolSession,
    ReceivedBodyState, TypeSet,
};
use trillium_server_common::{
    ArcedConnector, ArcedQuicClientConfig, Connector, QuicClientConfig, Transport,
    url::{Origin, Url},
};

/// Default maximum idle time for a pooled HTTP/1.1 connection. A warm keepalive connection is
/// worth holding onto — reusing it saves a full TCP (and, for https, TLS) handshake — so the
/// timeout exists only to bound *unbounded* retention, not to reclaim connections eagerly. A
/// connection the origin closed first is discarded cheaply either way: by the reuse-time
/// liveness probe when it's next reached for, or by the background reaper. Matches the h2
/// default for the same reason.
const DEFAULT_H1_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Default maximum idle time for a pooled HTTP/2 connection. Longer than h1 because the
/// initial h2 handshake (TCP + TLS + ALPN + SETTINGS exchange) is more expensive to
/// re-establish.
const DEFAULT_H2_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Default idle threshold above which a pooled HTTP/2 connection is liveness-pinged before
/// being handed out for a new request. Below this, we trust the connection without probing.
const DEFAULT_H2_IDLE_PING_THRESHOLD: Duration = Duration::from_secs(10);

/// Default timeout for the liveness PING — if we don't get an ACK within this window, the
/// connection is treated as dead and a fresh one is established instead.
const DEFAULT_H2_IDLE_PING_TIMEOUT: Duration = Duration::from_secs(20);

const DEFAULT_MAX_BUFFERED_REQUEST_BODY: usize = 1024;

/// Default time to wait for a `100 (Continue)` interim response before sending the request body
/// anyway. Matches curl's fallback.
const DEFAULT_EXPECT_CONTINUE_TIMEOUT: Duration = Duration::from_secs(1);

/// An HTTP client supporting HTTP/1.x, HTTP/2 (via ALPN), and — when configured with a QUIC
/// implementation — HTTP/3. See [`Client::new`] and [`Client::new_with_quic`] for construction
/// information.
#[derive(Clone, Debug, fieldwork::Fieldwork)]
pub struct Client {
    config: ArcedConnector,

    #[field(vis = "pub(crate)", get)]
    h3: Option<H3ClientState>,

    #[field(vis = "pub(crate)", get)]
    pool: Option<Pool<Origin, Box<dyn Transport>>>,

    #[field(vis = "pub(crate)", get)]
    h2_pool: Option<Pool<Origin, H2Pooled>>,

    /// Maximum idle time for a pooled HTTP/1.1 connection before a background reaper closes it and
    /// removes it from the pool. `None` disables expiry, restoring the previous behavior in which
    /// idle h1 connections were retained until reused (and discarded by a liveness probe) or the
    /// pool was manually cleaned up — an origin that stopped being contacted then held its idle
    /// file descriptors indefinitely.
    ///
    /// Defaults to 5 minutes.
    #[field(get, set, with, without, copy)]
    h1_idle_timeout: Option<Duration>,

    /// Maximum idle time for a pooled HTTP/2 connection. `None` disables expiry.
    ///
    /// Defaults to 5 minutes.
    #[field(get, set, with, without, copy)]
    h2_idle_timeout: Option<Duration>,

    /// If a pooled HTTP/2 connection has been idle for longer than this, an active PING is
    /// sent to verify it's still alive before being handed out. `None` disables the probe.
    ///
    /// Defaults to 10 seconds.
    #[field(get, set, with, copy, without)]
    h2_idle_ping_threshold: Option<Duration>,

    /// Timeout for the liveness PING sent under the [`h2_idle_ping_threshold`] policy.
    /// Connections whose ACK doesn't arrive within this window are treated as dead.
    ///
    /// Defaults to 20 seconds.
    ///
    /// [`h2_idle_ping_threshold`]: Self::h2_idle_ping_threshold
    #[field(get, set, with, copy)]
    h2_idle_ping_timeout: Duration,

    /// url base for this client
    #[field(get)]
    base: Option<Arc<Url>>,

    /// default request headers
    #[field(get)]
    default_headers: Arc<Headers>,

    /// Request bodies of unknown length at or below this size are fully buffered before the
    /// request head is written, letting the client send them with an accurate `Content-Length`.
    /// Two consequences follow from being able to buffer: a small streaming body is framed with
    /// `Content-Length` rather than `Transfer-Encoding: chunked`, and no `Expect: 100-continue`
    /// handshake is used — there is no benefit to asking permission before sending a body we
    /// have already buffered and can send in one shot. Larger or unbounded bodies stream
    /// (chunked), and the `Expect: 100-continue` handshake applies to those. A known-length
    /// body larger than this also uses `Expect: 100-continue`.
    ///
    /// Defaults to 1 KiB.
    #[field(get, set, with, copy)]
    max_buffered_request_body: usize,

    /// How long to wait for a `100 (Continue)` interim response after sending a request head
    /// carrying `Expect: 100-continue`, before sending the request body regardless.
    ///
    /// Per [RFC 9110 §10.1.1] a client must not wait indefinitely: a `100 (Continue)` cannot
    /// traverse an HTTP/1.0 intermediary, and not every peer honors the expectation, so a client
    /// that waited forever would deadlock against them. On timeout the body is sent anyway — the
    /// same thing the client would do without the expectation.
    ///
    /// Defaults to 1 second.
    ///
    /// [RFC 9110 §10.1.1]: https://www.rfc-editor.org/rfc/rfc9110#section-10.1.1
    #[field(get, set, with, copy)]
    expect_continue_timeout: Duration,

    /// optional per-request timeout
    #[field(get, set, with, copy, without, option_set_some)]
    timeout: Option<Duration>,

    /// configuration
    #[field(get, get_mut, set, with, into)]
    context: Arc<HttpContext>,

    /// type-erased middleware stack. Defaults to a no-op `()` handler. Set via
    /// [`Client::with_handler`] / [`Client::set_handler`]; recover the concrete type via
    /// [`Client::downcast_handler`].
    #[field(vis = "pub(crate)", get = arc_handler)]
    handler: ArcedClientHandler,

    /// Encrypted-DNS resolver, if configured via [`Client::with_doh`]. When set, all DNS
    /// resolution for this client is routed through it.
    #[cfg(feature = "hickory")]
    pub(crate) resolver: Option<crate::dns::Resolver>,
}

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        method!(
            $fn_name,
            $method,
            concat!(
                // yep, macro-generated doctests
                "Builds a new client conn with the ",
                stringify!($fn_name),
                " http method and the provided url.

```
use trillium_client::{Client, Method};
use trillium_testing::client_config;

let client = Client::new(client_config());
let conn = client.",
                stringify!($fn_name),
                "(\"http://localhost:8080/some/route\"); //<-

assert_eq!(conn.method(), Method::",
                stringify!($method),
                ");
assert_eq!(conn.url().to_string(), \"http://localhost:8080/some/route\");
```
"
            )
        );
    };

    ($fn_name:ident, $method:ident, $doc_comment:expr_2021) => {
        #[doc = $doc_comment]
        pub fn $fn_name(&self, url: impl IntoUrl) -> Conn {
            self.build_conn(Method::$method, url)
        }
    };
}

pub(crate) fn default_request_headers() -> Headers {
    Headers::new()
        .with_inserted_header(KnownHeaderName::UserAgent, USER_AGENT)
        .with_inserted_header(KnownHeaderName::Accept, "*/*")
}

impl Client {
    method!(get, Get);

    method!(post, Post);

    method!(put, Put);

    method!(delete, Delete);

    method!(patch, Patch);

    /// builds a new client from this `Connector`
    pub fn new(connector: impl Connector) -> Self {
        let config = ArcedConnector::new(connector);
        let (pool, h2_pool) = (Pool::default(), Pool::default());
        crate::reaper::spawn_pool_reaper(config.runtime(), pool.downgrade(), h2_pool.downgrade());
        Self {
            config,
            h3: None,
            pool: Some(pool),
            h2_pool: Some(h2_pool),
            h1_idle_timeout: Some(DEFAULT_H1_IDLE_TIMEOUT),
            h2_idle_timeout: Some(DEFAULT_H2_IDLE_TIMEOUT),
            h2_idle_ping_threshold: Some(DEFAULT_H2_IDLE_PING_THRESHOLD),
            h2_idle_ping_timeout: DEFAULT_H2_IDLE_PING_TIMEOUT,
            max_buffered_request_body: DEFAULT_MAX_BUFFERED_REQUEST_BODY,
            expect_continue_timeout: DEFAULT_EXPECT_CONTINUE_TIMEOUT,
            base: None,
            default_headers: Arc::new(default_request_headers()),
            timeout: None,
            context: Default::default(),
            handler: ArcedClientHandler::new(()),
            #[cfg(feature = "hickory")]
            resolver: None,
        }
    }

    /// Build a new client with both a TCP connector and a QUIC connector for HTTP/3 support.
    ///
    /// The connector's runtime and UDP socket type are bound to the QUIC connector here,
    /// before type erasure, so that `trillium-quinn` and the runtime adapter remain
    /// independent crates that neither depends on the other.
    ///
    /// When H3 is configured, the client will track `Alt-Svc` headers in responses and
    /// automatically use HTTP/3 for subsequent requests to origins that advertise it.
    /// Other requests follow the standard h1 / h2-via-ALPN path.
    pub fn new_with_quic<C: Connector, Q: QuicClientConfig<C>>(connector: C, quic: Q) -> Self {
        // Bind the runtime into the QUIC client config before consuming `connector`.
        let arced_quic = ArcedQuicClientConfig::new(&connector, quic);

        #[cfg_attr(not(feature = "webtransport"), allow(unused_mut))]
        let mut context = HttpContext::default();
        #[cfg(feature = "webtransport")]
        {
            // Advertise WebTransport-over-h3 capability on outbound SETTINGS so a server can
            // open server-initiated WT streams to us once a session is established.
            // ENABLE_CONNECT_PROTOCOL is included for symmetry with the server side; harmless
            // when the client never receives extended-CONNECT from the peer.
            context
                .config_mut()
                .set_h3_datagrams_enabled(true)
                .set_webtransport_enabled(true)
                .set_extended_connect_enabled(true);
        }

        let config = ArcedConnector::new(connector);
        let (pool, h2_pool) = (Pool::default(), Pool::default());
        crate::reaper::spawn_pool_reaper(config.runtime(), pool.downgrade(), h2_pool.downgrade());
        Self {
            config,
            h3: Some(H3ClientState::new(arced_quic)),
            pool: Some(pool),
            h2_pool: Some(h2_pool),
            h1_idle_timeout: Some(DEFAULT_H1_IDLE_TIMEOUT),
            h2_idle_timeout: Some(DEFAULT_H2_IDLE_TIMEOUT),
            h2_idle_ping_threshold: Some(DEFAULT_H2_IDLE_PING_THRESHOLD),
            h2_idle_ping_timeout: DEFAULT_H2_IDLE_PING_TIMEOUT,
            max_buffered_request_body: DEFAULT_MAX_BUFFERED_REQUEST_BODY,
            expect_continue_timeout: DEFAULT_EXPECT_CONTINUE_TIMEOUT,
            base: None,
            default_headers: Arc::new(default_request_headers()),
            timeout: None,
            context: Arc::new(context),
            handler: ArcedClientHandler::new(()),
            #[cfg(feature = "hickory")]
            resolver: None,
        }
    }

    /// Install a [`ClientHandler`] middleware stack on this client.
    ///
    /// The handler runs around every request issued by this client: its `run` method fires before
    /// the network round-trip (with the option to halt + synthesize a response), and its
    /// `after_response` fires afterwards. Compose multiple handlers with tuples — see
    /// [`ClientHandler`] for the lifecycle and `Vec`/tuple/`Option` impls.
    ///
    /// Returns `self` for chaining.
    #[must_use]
    pub fn with_handler<H: ClientHandler>(mut self, handler: H) -> Self {
        self.set_handler(handler);
        self
    }

    /// Install a [`ClientHandler`] middleware stack on this client. See [`Client::with_handler`]
    /// for details.
    pub fn set_handler<H: ClientHandler>(&mut self, handler: H) -> &mut Self {
        self.handler = ArcedClientHandler::new(handler);
        self
    }

    /// Borrow the type-erased [`ClientHandler`]
    ///
    /// See also [`Client::downcast_handler`] if you can name the type
    pub fn handler(&self) -> &impl ClientHandler {
        &self.handler
    }

    /// Borrow the installed [`ClientHandler`] as the concrete type `T`, returning `None` if the
    /// installed handler is not of that type.
    ///
    /// Useful for inspecting handler-internal state from outside the request path — e.g., reading
    /// counters from a metrics handler.
    pub fn downcast_handler<T: Any + 'static>(&self) -> Option<&T> {
        self.handler.downcast_ref()
    }

    /// chainable method to remove a header from default request headers
    pub fn without_default_header(mut self, name: impl Into<HeaderName<'static>>) -> Self {
        self.default_headers_mut().remove(name);
        self
    }

    /// chainable method to insert a new default request header, replacing any existing value
    pub fn with_default_header(
        mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        self.default_headers_mut().insert(name, value);
        self
    }

    /// borrow the default headers mutably
    ///
    /// calling this will copy-on-write if the default headers are shared with another client clone
    pub fn default_headers_mut(&mut self) -> &mut Headers {
        Arc::make_mut(&mut self.default_headers)
    }

    /// chainable constructor to disable http/1.1 connection reuse.
    ///
    /// ```
    /// use trillium_client::Client;
    /// use trillium_smol::ClientConfig;
    ///
    /// let client = Client::new(ClientConfig::default()).without_keepalive();
    /// ```
    pub fn without_keepalive(mut self) -> Self {
        self.pool = None;
        self.h2_pool = None;
        self
    }

    /// builds a new conn.
    ///
    /// if the client has pooling enabled and there is an available connection for this
    /// origin (scheme + host + port), the new conn will reuse it when sent.
    ///
    /// ```
    /// use trillium_client::{Client, Method};
    /// use trillium_smol::ClientConfig;
    /// let client = Client::new(ClientConfig::default());
    ///
    /// let conn = client.build_conn("get", "http://trillium.rs"); //<-
    ///
    /// assert_eq!(conn.method(), Method::Get);
    /// assert_eq!(conn.url().host_str().unwrap(), "trillium.rs");
    /// ```
    pub fn build_conn<M>(&self, method: M, url: impl IntoUrl) -> Conn
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
    {
        let method = method.try_into().unwrap();
        let (url, request_target, error) = if let Some(base) = &self.base
            && let Some(request_target) = url.request_target(method)
        {
            ((**base).clone(), Some(request_target), None)
        } else {
            match self.build_url(url) {
                Ok(url) => (url, None, None),
                // `build_conn` is infallible by contract, so a malformed url is
                // deferred rather than panicked: stash the error (surfaced at the
                // top of `Conn::exec`) behind a placeholder url that never dials.
                Err(error) => (
                    Url::parse("http://invalid.invalid/").expect("literal is a valid url"),
                    None,
                    Some(error),
                ),
            }
        };

        Conn {
            url,
            method,
            request_headers: Headers::clone(&self.default_headers),
            response_headers: Headers::new(),
            transport: None,
            status: None,
            request_body: None,
            request_body_fully_buffered: false,
            protocol_session: ProtocolSession::Http1,
            #[cfg(feature = "webtransport")]
            wt_pool_entry: None,
            buffer: Vec::with_capacity(128).into(),
            response_body_state: ReceivedBodyState::End,
            headers_finalized: false,
            halted: false,
            error,
            body_override: None,
            timeout: self.timeout,
            http_version: None,
            max_head_length: 8 * 1024,
            state: TypeSet::new(),
            context: self.context.clone(),
            authority: None,
            scheme: None,
            path: None,
            request_target,
            protocol: None,
            request_trailers: None,
            response_trailers: None,
            client: self.clone(),
            followup: None,
            upgrade: false,
        }
    }

    /// borrow the connector for this client
    pub fn connector(&self) -> &ArcedConnector {
        &self.config
    }

    /// The pool implementation accumulates a small memory footprint for each new host. If
    /// your application is reusing a pool against a large number of unique hosts, call this
    /// method intermittently.
    pub fn clean_up_pool(&self) {
        if let Some(pool) = &self.pool {
            pool.reap();
        }
        if let Some(h2_pool) = &self.h2_pool {
            h2_pool.reap();
        }
    }

    /// chainable method to set the base for this client
    pub fn with_base(mut self, base: impl IntoUrl) -> Self {
        self.set_base(base).unwrap();
        self
    }

    /// attempt to build a url from this IntoUrl and the [`Client::base`], if set
    pub fn build_url(&self, url: impl IntoUrl) -> crate::Result<Url> {
        url.into_url(self.base())
    }

    /// set the base for this client
    pub fn set_base(&mut self, base: impl IntoUrl) -> crate::Result<()> {
        let mut base = base.into_url(None)?;

        if !base.path().ends_with('/') {
            log::warn!("appending a trailing / to {base}");
            base.set_path(&format!("{}/", base.path()));
        }

        self.base = Some(Arc::new(base));
        Ok(())
    }

    /// Mutate the url base for this client.
    ///
    /// This has "clone-on-write" semantics if there are other clones of this client. If there are
    /// other clones of this client, they will not be updated.
    pub fn base_mut(&mut self) -> Option<&mut Url> {
        let base = self.base.as_mut()?;
        Some(Arc::make_mut(base))
    }
}

impl<T: Connector> From<T> for Client {
    fn from(connector: T) -> Self {
        Self::new(connector)
    }
}
