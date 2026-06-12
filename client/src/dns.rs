//! DNS resolution over an encrypted transport, including SVCB and HTTPS resource
//! records ([RFC 9460]). DNS-over-HTTPS (DoH, [RFC 8484]), DNS-over-TLS (DoT,
//! [RFC 7858]), and DNS-over-QUIC (DoQ, [RFC 9250]) are supported; the [`Resolver`] /
//! [`DnsTransport`] split keeps the shared query/cache/SVCB core ([`codec`])
//! independent of the transport, so each transport is just one [`DnsTransport`]
//! variant plus its own `exchange`.
//!
//! `hickory-proto` is used purely as a wire codec — building the query message and
//! parsing the response (see [`codec`]). The actual IO is performed by [`Client`]
//! itself (DoH over its own pool, DoT/DoQ over the connector), so this path inherits
//! the client's runtime and TLS stack and pulls in no resolver of its own.
//!
//! Resolutions are kept in a [`DnsCache`] that is independent of the connection
//! pool: evicting a pooled connection does not invalidate a still-live
//! resolution, and a single resolution is shared across HTTP/1, HTTP/2, and
//! HTTP/3 — so an origin reachable by Alt-Svc but without SVCB still need only
//! be resolved once.
//!
//! [RFC 7858]: https://www.rfc-editor.org/rfc/rfc7858
//! [RFC 8484]: https://www.rfc-editor.org/rfc/rfc8484
//! [RFC 9250]: https://www.rfc-editor.org/rfc/rfc9250
//! [RFC 9460]: https://www.rfc-editor.org/rfc/rfc9460
//!
//! [`Client`]: crate::Client

mod codec;
mod doh;
mod doq;
mod dot;
mod framing;

use crate::Client;
use async_lock::OnceCell;
pub(crate) use codec::Resolved;
use codec::{build_query, parse_response};
use dashmap::DashMap;
use doh::Doh;
use doq::Doq;
use dot::Dot;
use futures_lite::future;
use hickory_proto::rr::RecordType;
use std::{
    future::Future,
    io::{self, ErrorKind},
    sync::Arc,
    time::{Duration, Instant},
};
use trillium_http::Version;
use trillium_server_common::{Connector, url::Url};

/// Cache lifetime for the resolver endpoint's own bootstrap resolution, which
/// comes from the system resolver rather than a DNS record with a TTL.
const BOOTSTRAP_TTL: Duration = Duration::from_secs(300);

/// TTL floor — DNS records with shorter TTLs are cached this long anyway, to
/// avoid re-resolving on every request when an origin publishes a near-zero TTL.
const MIN_TTL: Duration = Duration::from_secs(1);

/// TTL ceiling — caps how stale a cached resolution can become regardless of the
/// record's advertised TTL.
const MAX_TTL: Duration = Duration::from_secs(3600);

/// DNS resolution timeout when the request carries no overall timeout of its own. Bounds the whole
/// resolution (connect plus the A/AAAA/HTTPS exchanges), so an unreachable resolver — or one that
/// doesn't speak the configured transport at all, like a DoT host addressed over DoQ — surfaces as
/// a clear DNS error rather than hanging indefinitely.
const DEFAULT_DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// When the request *does* carry an overall timeout, DNS gets at most this fraction of it in the
/// worst case, leaving the rest of the budget for the connection and response. DNS is normally
/// fast, so this only matters when a resolver stalls: it fails the lookup with a clear DNS error
/// roughly halfway through the budget rather than letting it consume the whole thing and surface as
/// a generic request timeout.
fn dns_timeout(request_timeout: Option<Duration>) -> Duration {
    request_timeout.map_or(DEFAULT_DNS_TIMEOUT, |timeout| timeout / 2)
}

/// A host-keyed DNS cache shared across protocols and independent of the
/// connection pool. Cheaply cloneable (Arc-backed).
#[derive(Debug, Clone, Default)]
pub(crate) struct DnsCache {
    entries: Arc<DashMap<Box<str>, CacheEntry>>,
    /// Per-host single-flight slots. While one resolution is in flight, concurrent resolves of
    /// the same host await its result instead of each issuing their own A/AAAA/HTTPS queries.
    /// Independent of `entries` (the TTL'd result cache) and reaped as resolutions complete.
    in_flight: Arc<DashMap<Box<str>, Arc<OnceCell<Resolved>>>>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    resolved: Resolved,
    expiry: Instant,
}

impl DnsCache {
    /// Return the cached resolution for `host` if present and unexpired,
    /// evicting it if it has expired.
    pub(crate) fn get(&self, host: &str) -> Option<Resolved> {
        let expired = {
            let entry = self.entries.get(host)?;
            if entry.expiry >= Instant::now() {
                return Some(entry.resolved.clone());
            }
            true
        };
        if expired {
            self.entries.remove(host);
        }
        None
    }

    /// Cache `resolved` for `host`, honoring `ttl` clamped to [`MIN_TTL`,
    /// `MAX_TTL`].
    pub(crate) fn insert(&self, host: &str, resolved: Resolved, ttl: Duration) {
        let expiry = Instant::now() + ttl.clamp(MIN_TTL, MAX_TTL);
        self.entries
            .insert(host.into(), CacheEntry { resolved, expiry });
    }

    /// Resolve `host` through `query`, coalescing concurrent resolutions of the same host so a
    /// burst issues one set of DNS queries rather than one per caller. The winning caller runs
    /// `query` and populates the TTL cache; the rest await its result. On error each caller falls
    /// back to its own attempt, so a transient failure neither poisons the cache nor wedges the
    /// waiters.
    pub(crate) async fn resolve_coalesced(
        &self,
        host: &str,
        query: impl Future<Output = io::Result<(Resolved, Duration)>>,
    ) -> io::Result<Resolved> {
        if let Some(hit) = self.get(host) {
            return Ok(hit);
        }

        let cell = self
            .in_flight
            .entry(host.into())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        // `get_or_try_init` runs `query` on exactly one caller; the rest await the same cell. If
        // it errors (or the running future is cancelled), the cell stays uninitialized and another
        // caller retries — no guard needed to avoid a wedged slot.
        let resolved = cell
            .get_or_try_init(|| async {
                let (resolved, ttl) = query.await?;
                self.insert(host, resolved.clone(), ttl);
                Ok::<_, io::Error>(resolved)
            })
            .await
            .cloned();

        // Best-effort reap: holders of a clone of `cell` already have the value, and a leaked
        // empty cell (every caller cancelled) self-heals on the next resolve.
        self.in_flight.remove(host);
        resolved
    }
}

/// A configured DNS resolver: the transport that carries queries plus the cache that every
/// resolution through it populates.
///
/// Query construction, caching, SVCB handling, and the A/AAAA/HTTPS orchestration are all
/// transport-independent and live here; a [`DnsTransport`] variant supplies only the wire
/// exchange and its own resolver host. Cheaply cloneable — the cache is Arc-backed, so clones
/// share it.
#[derive(Debug, Clone)]
pub(crate) struct Resolver {
    cache: DnsCache,
    transport: DnsTransport,
}

/// The mechanism a [`Resolver`] uses to turn a wire-format query into a wire-format response.
/// The single seam where a resolution touches the network; everything around it is shared.
#[derive(Debug, Clone)]
enum DnsTransport {
    /// DNS-over-HTTPS: queries POST over the client's own connection pool.
    Doh(Doh),
    /// DNS-over-TLS: queries pipeline over a persistent TLS connection to the resolver.
    Dot(Dot),
    /// DNS-over-QUIC: each query rides its own bidi stream on a cached QUIC connection.
    Doq(Doq),
}

impl Resolver {
    /// Build a DoH resolver pointing at `resolver`.
    pub(crate) fn doh(resolver: Url) -> Self {
        Self {
            cache: DnsCache::default(),
            transport: DnsTransport::Doh(Doh::new(resolver, None)),
        }
    }

    /// Build a DoH resolver pointing at `resolver`, pinning the connection to it to HTTP/3.
    pub(crate) fn doh3(resolver: Url) -> Self {
        Self {
            cache: DnsCache::default(),
            transport: DnsTransport::Doh(Doh::new(resolver, Some(Version::Http3))),
        }
    }

    /// Build a DoT resolver pointing at `resolver`.
    pub(crate) fn dot(resolver: Url) -> Self {
        Self {
            cache: DnsCache::default(),
            transport: DnsTransport::Dot(Dot::new(resolver)),
        }
    }

    /// Build a DoQ resolver pointing at `resolver`.
    pub(crate) fn doq(resolver: Url) -> Self {
        Self {
            cache: DnsCache::default(),
            transport: DnsTransport::Doq(Doq::new(resolver)),
        }
    }

    /// Resolve `host:port` through the resolver, caching and returning the result.
    ///
    /// The resolver endpoint's own host is the single name resolved via the connector's system
    /// resolver (it can't be looked up over itself); every other name is resolved over the
    /// configured transport, so once a client opts in, its lookups don't reach the local/system
    /// resolver at all.
    pub(crate) async fn resolve(
        &self,
        client: &Client,
        host: &str,
        port: u16,
        request_timeout: Option<Duration>,
    ) -> io::Result<Resolved> {
        let kind = self.transport.kind();
        let endpoint = self.transport.resolver_endpoint();
        let timeout = dns_timeout(request_timeout);
        log::debug!("resolving {host}:{port} via {kind} ({endpoint})");
        // Box the query future: resolving a host issues client requests that themselves resolve
        // DNS (the resolver's own host), so this nests, and the query future holds the resolver
        // `Conn`s inline. Boxing keeps that frame off the stack so the recursion can't overflow it.
        let resolved = client
            .connector()
            .runtime()
            .timeout(
                timeout,
                self.cache
                    .resolve_coalesced(host, Box::pin(self.query_host(client, host, port))),
            )
            .await
            .unwrap_or_else(|| {
                Err(io::Error::new(
                    ErrorKind::TimedOut,
                    format!(
                        "{kind} resolution of {host} via {endpoint} timed out after {timeout:?}; \
                         the resolver may be unreachable or may not speak {kind}"
                    ),
                ))
            });
        match &resolved {
            Ok(r) => log::debug!(
                "resolved {host} to {} address(es), {} service binding(s)",
                r.addrs.len(),
                r.services.len()
            ),
            Err(e) => log::debug!("resolution of {host} failed: {e}"),
        }
        resolved
    }

    /// Issue the DNS queries for `host:port` and assemble the resolution plus its cache lifetime.
    /// Does not touch the cache — coalescing and caching are the caller's concern.
    async fn query_host(
        &self,
        client: &Client,
        host: &str,
        port: u16,
    ) -> io::Result<(Resolved, Duration)> {
        // The resolver's own host can't be looked up over itself, so it's resolved via the
        // connector's system resolver instead (or given as an IP, skipping even that). Its TTL is
        // a fixed bootstrap window since the system answer carries none.
        if self.transport.resolver_host() == Some(host) {
            let addrs = client
                .connector()
                .resolve(host, port)
                .await?
                .into_iter()
                .map(|addr| addr.ip())
                .collect();
            return Ok((
                Resolved {
                    addrs,
                    services: Vec::new(),
                },
                BOOTSTRAP_TTL,
            ));
        }

        // A, AAAA, and HTTPS are separate questions (resolvers answer only the
        // first question of a message), issued concurrently over the transport.
        let (a, (aaaa, https)) = future::try_zip(
            self.query(client, build_query(host, port, RecordType::A)?),
            future::try_zip(
                self.query(client, build_query(host, port, RecordType::AAAA)?),
                self.query(client, build_query(host, port, RecordType::HTTPS)?),
            ),
        )
        .await?;

        let mut resolved = Resolved::default();
        let mut min_ttl = MAX_TTL;
        for (part, ttl) in [a, aaaa, https] {
            resolved.merge(part);
            min_ttl = min_ttl.min(ttl);
        }
        resolved.services.sort_by_key(|s| s.priority);

        if !resolved.has_addrs() {
            return Err(io::Error::new(
                ErrorKind::NotFound,
                format!("DNS resolver returned no addresses for {host}"),
            ));
        }

        Ok((resolved, min_ttl))
    }

    /// Exchange a single wire-format query for a response over the transport and parse it.
    async fn query(&self, client: &Client, query: Vec<u8>) -> io::Result<(Resolved, Duration)> {
        let bytes = self.transport.exchange(client, query).await?;
        parse_response(&bytes)
    }
}

impl DnsTransport {
    /// A short transport label for diagnostics.
    fn kind(&self) -> &'static str {
        match self {
            DnsTransport::Doh(_) => "DoH",
            DnsTransport::Dot(_) => "DoT",
            DnsTransport::Doq(_) => "DoQ",
        }
    }

    /// The resolver endpoint URL, for diagnostics.
    fn resolver_endpoint(&self) -> &Url {
        match self {
            DnsTransport::Doh(doh) => doh.resolver(),
            DnsTransport::Dot(dot) => dot.resolver(),
            DnsTransport::Doq(doq) => doq.resolver(),
        }
    }

    /// The resolver's own host — the one name bootstrapped via the connector rather than resolved
    /// over the transport. `None` when the resolver is given as a bare IP.
    fn resolver_host(&self) -> Option<&str> {
        match self {
            DnsTransport::Doh(doh) => doh.host(),
            DnsTransport::Dot(dot) => dot.host(),
            DnsTransport::Doq(doq) => doq.host(),
        }
    }

    /// Carry one wire-format query to the resolver and return the wire-format response bytes.
    async fn exchange(&self, client: &Client, query: Vec<u8>) -> io::Result<Vec<u8>> {
        match self {
            DnsTransport::Doh(doh) => doh.exchange(client, query).await,
            DnsTransport::Dot(dot) => dot.exchange(client, query).await,
            DnsTransport::Doq(doq) => doq.exchange(client, query).await,
        }
    }
}

impl Client {
    /// Build a DoH resolver URL from a full URL or a bare host/IP. A missing scheme defaults to
    /// `https`, and a missing path defaults to `/dns-query` — the ubiquitous convention, though
    /// [RFC 8484] leaves the path to out-of-band configuration, so an explicit path is honored.
    ///
    /// [RFC 8484]: https://www.rfc-editor.org/rfc/rfc8484
    fn doh_resolver_url(resolver: &str) -> Url {
        let mut url = if resolver.contains("://") {
            Url::parse(resolver)
        } else {
            Url::parse(&format!("https://{resolver}"))
        }
        .expect("DoH resolver must be a valid URL or host");
        if matches!(url.path(), "" | "/") {
            url.set_path("/dns-query");
        }
        url
    }

    /// Assign the client's single encrypted-DNS resolver, warning if one was already configured.
    /// A client holds one resolver, so a later DNS configurator replaces an earlier one — almost
    /// always a mistake rather than an intent.
    fn set_resolver(&mut self, resolver: Resolver) {
        if self.resolver.is_some() {
            log::warn!(
                "replacing an already-configured DNS resolver; encrypted-DNS resolvers are \
                 mutually exclusive"
            );
        }
        self.resolver = Some(resolver);
    }

    /// Route all DNS resolution for this client through the given DNS-over-HTTPS
    /// ([RFC 8484]) resolver, including SVCB/HTTPS records ([RFC 9460]).
    ///
    /// `resolver` may be a full URL (`https://1.1.1.1/dns-query`) or a bare host or IP (`1.1.1.1`).
    /// A missing scheme defaults to `https` and a missing path to `/dns-query`, so `1.1.1.1`,
    /// `https://1.1.1.1`, and `https://1.1.1.1/dns-query` are equivalent; an explicit path is
    /// honored, since [RFC 8484] leaves the path to out-of-band configuration. An IP avoids any
    /// bootstrap lookup; a hostname is resolved once via the underlying connector and then cached
    /// like any other name.
    ///
    /// A client holds a single DNS resolver; calling [`with_doh3`](Client::with_doh3),
    /// [`with_dot`](Client::with_dot), or [`with_doq`](Client::with_doq) after this replaces it and
    /// logs a warning.
    ///
    /// # Panics
    ///
    /// Panics if `resolver` is neither a valid URL nor a valid host.
    ///
    /// [RFC 8484]: https://www.rfc-editor.org/rfc/rfc8484
    /// [RFC 9460]: https://www.rfc-editor.org/rfc/rfc9460
    #[must_use]
    pub fn with_doh(mut self, resolver: impl AsRef<str>) -> Self {
        let url = Self::doh_resolver_url(resolver.as_ref());
        self.set_resolver(Resolver::doh(url));
        self
    }

    /// Route all DNS resolution for this client through the given DNS-over-HTTPS
    /// ([RFC 8484]) resolver, forcing the connection to the resolver itself onto HTTP/3.
    ///
    /// Identical to [`with_doh`](Client::with_doh) except that the connection to the resolver is
    /// pinned to HTTP/3 rather than negotiated (h1/h2) over ALPN. Use this for resolvers that serve
    /// DoH over HTTP/3 but don't advertise it via [`Alt-Svc`][altsvc] — which would otherwise leave
    /// the client on h2 indefinitely. Only the resolver connection is affected; requests to
    /// resolved hosts pick their protocol from SVCB/Alt-Svc as usual.
    ///
    /// A client holds a single DNS resolver; calling [`with_doh`](Client::with_doh),
    /// [`with_dot`](Client::with_dot), or [`with_doq`](Client::with_doq) after this replaces it and
    /// logs a warning.
    ///
    /// # Panics
    ///
    /// Panics if the client is not HTTP/3-capable (build it with [`Client::new_with_quic`]), or if
    /// `resolver` is neither a valid URL nor a valid host.
    ///
    /// [RFC 8484]: https://www.rfc-editor.org/rfc/rfc8484
    /// [altsvc]: https://www.rfc-editor.org/rfc/rfc7838
    #[must_use]
    pub fn with_doh3(mut self, resolver: impl AsRef<str>) -> Self {
        assert!(
            self.h3().is_some(),
            "with_doh3 requires an HTTP/3-capable client; build it with Client::new_with_quic"
        );
        let url = Self::doh_resolver_url(resolver.as_ref());
        self.set_resolver(Resolver::doh3(url));
        self
    }

    /// Route all DNS resolution for this client through the given DNS-over-TLS
    /// ([RFC 7858]) resolver, including SVCB/HTTPS records ([RFC 9460]).
    ///
    /// `resolver` may be a full `https://` URL or a bare host or IP (`1.1.1.1`), which expands to
    /// `https://<host>:853` — the registered DoT port. An IP avoids any bootstrap lookup; a
    /// hostname is resolved once via the underlying connector and then cached like any other name.
    ///
    /// Each lookup opens a one-shot TLS connection to the resolver, so the client must be
    /// configured with a TLS connector; a plaintext connector fails the lookup (and, because
    /// resolution is fail-closed, the request) rather than falling back to the system resolver.
    ///
    /// A client holds a single DNS resolver; calling [`with_doh`](Client::with_doh),
    /// [`with_doh3`](Client::with_doh3), or [`with_doq`](Client::with_doq) after this replaces it
    /// and logs a warning.
    ///
    /// # Panics
    ///
    /// Panics if `resolver` is neither a valid URL nor a valid host.
    ///
    /// [RFC 7858]: https://www.rfc-editor.org/rfc/rfc7858
    /// [RFC 9460]: https://www.rfc-editor.org/rfc/rfc9460
    #[must_use]
    pub fn with_dot(mut self, resolver: impl AsRef<str>) -> Self {
        let resolver = resolver.as_ref();
        let url = if resolver.contains("://") {
            Url::parse(resolver)
        } else {
            Url::parse(&format!("https://{resolver}:853"))
        }
        .expect("with_dot requires a valid resolver host or URL");
        self.set_resolver(Resolver::dot(url));
        self
    }

    /// Route all DNS resolution for this client through the given DNS-over-QUIC
    /// ([RFC 9250]) resolver, including SVCB/HTTPS records ([RFC 9460]).
    ///
    /// `resolver` may be a full `https://` URL or a bare host or IP (`1.1.1.1`), which expands to
    /// `https://<host>:853` — the registered DoQ port. An IP avoids any bootstrap lookup; a
    /// hostname is resolved once via the underlying connector and then cached like any other name.
    ///
    /// Queries ride a cached, multiplexed QUIC connection (one bidirectional stream per query)
    /// established over this client's HTTP/3 UDP endpoint with the `doq` ALPN, independent of the
    /// HTTP/3 connection pool. Resolution is fail-closed, like [`with_doh`](Client::with_doh).
    ///
    /// A client holds a single DNS resolver; calling [`with_doh`](Client::with_doh),
    /// [`with_doh3`](Client::with_doh3), or [`with_dot`](Client::with_dot) after this replaces it
    /// and logs a warning.
    ///
    /// # Panics
    ///
    /// Panics if the client is not HTTP/3-capable (build it with [`Client::new_with_quic`]), or if
    /// `resolver` is neither a valid URL nor a valid host.
    ///
    /// [RFC 9250]: https://www.rfc-editor.org/rfc/rfc9250
    /// [RFC 9460]: https://www.rfc-editor.org/rfc/rfc9460
    #[must_use]
    pub fn with_doq(mut self, resolver: impl AsRef<str>) -> Self {
        assert!(
            self.h3().is_some(),
            "with_doq requires an HTTP/3-capable client; build it with Client::new_with_quic"
        );
        let resolver = resolver.as_ref();
        let url = if resolver.contains("://") {
            Url::parse(resolver)
        } else {
            Url::parse(&format!("https://{resolver}:853"))
        }
        .expect("with_doq requires a valid resolver host or URL");
        self.set_resolver(Resolver::doq(url));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn cache_round_trips_and_expires() {
        let cache = DnsCache::default();
        let resolved = Resolved {
            addrs: vec![
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 9)),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ],
            services: Vec::new(),
        };

        cache.insert("example.com", resolved.clone(), Duration::from_secs(300));
        assert_eq!(cache.get("example.com").unwrap().addrs.len(), 2);
        assert!(cache.get("absent.example").is_none());

        // A zero TTL is floored to MIN_TTL, so it's briefly live rather than
        // immediately expired.
        cache.insert("floor.example", resolved, Duration::ZERO);
        assert!(cache.get("floor.example").is_some());
    }
}
