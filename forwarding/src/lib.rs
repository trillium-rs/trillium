//! # Trillium handler for `x-forwarded-*` / `forwarded`
//!
//! This simple handler rewrites the request's host, secure setting, and
//! peer ip based on headers added by a trusted reverse proxy.
//!
//! The specific headers that are understood by this handler are:
//!
//! [`Forwarded`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded)
//! or some combination of the following
//! - [`X-Forwarded-For`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-For)
//! - [`X-Forwarded-Proto`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-Proto)
//! - [`X-Forwarded-Host`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-Host)
//!
//! There are several ways of specifying when to trust a peer ip address,
//! and the narrowest possible trust rules should be used for a given
//! deployment so as to decrease the chance for a threat actor to generate
//! a request with forwarded headers that we mistakenly trust.
//!
//! Because the forwarded-for chain is append-only, only its trusted suffix is meaningful: the
//! peer ip is taken from the rightmost entry that is not itself a trusted proxy, walking right to
//! left. Everything to the left of that is under the control of whoever sent the request.
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

mod forwarded;
pub use forwarded::Forwarded;

mod parse_utils;

use std::{fmt::Debug, net::IpAddr, ops::Deref};
use trillium::{Conn, Handler, Status, Transport};

#[derive(Debug, Default)]
#[non_exhaustive]
enum TrustProxy {
    Always,

    #[default]
    Never,

    Cidr(Vec<cidr::AnyIpCidr>),

    Function(TrustFn),
}

struct TrustFn(Box<dyn Fn(&IpAddr) -> bool + Send + Sync + 'static>);
impl<F> From<F> for TrustFn
where
    F: Fn(&IpAddr) -> bool + Send + Sync + 'static,
{
    fn from(f: F) -> Self {
        Self(Box::new(f))
    }
}
impl Debug for TrustFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TrustPredicate")
            .field(&format_args!(".."))
            .finish()
    }
}

impl Deref for TrustFn {
    type Target = dyn Fn(&IpAddr) -> bool + Send + Sync + 'static;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TrustProxy {
    fn is_trusted(&self, ip: Option<IpAddr>) -> bool {
        match (self, ip) {
            (TrustProxy::Always, _) => true,
            (TrustProxy::Cidr(cidrs), Some(ip)) => cidrs.iter().any(|c| c.contains(&ip)),
            (TrustProxy::Function(trust_predicate), Some(ip)) => trust_predicate(&ip),
            _ => false,
        }
    }

    /// Walks the append-only forwarded-for chain from right to left, adopting each entry in turn
    /// and stopping at the first one that is not itself a trusted proxy.
    ///
    /// Everything to the left of the entry appended by the outermost trusted proxy is under the
    /// control of whoever sent the request, so only the trusted suffix of the chain may be
    /// traversed. Entries that do not parse as ip addresses (obfuscated identifiers, `unknown`)
    /// end the walk.
    fn rightmost_untrusted(
        &self,
        forwarded_for: &[&str],
        peer_ip: Option<IpAddr>,
    ) -> Option<IpAddr> {
        let mut peer_ip = peer_ip;
        for entry in forwarded_for.iter().rev() {
            let Some(ip) = parse_node(entry) else { break };
            peer_ip = Some(ip);
            if !self.is_trusted(peer_ip) {
                break;
            }
        }
        peer_ip
    }
}

/// Parses an RFC 7239 node identifier such as `192.0.2.60`, `192.0.2.60:8080`,
/// `[2001:db8::17]`, or `[2001:db8::17]:4711` as an ip address, discarding any port.
fn parse_node(node: &str) -> Option<IpAddr> {
    let node = node.trim();
    if let Some(rest) = node.strip_prefix('[') {
        return rest.split_once(']')?.0.parse().ok();
    }

    node.parse()
        .ok()
        .or_else(|| node.split_once(':')?.0.parse().ok())
}

/// Trillium handler for `forwarded`/`x-forwarded-*` headers
///
/// See crate-level docs for an explanation
#[derive(Default, Debug)]
pub struct Forwarding(TrustProxy);

impl From<TrustProxy> for Forwarding {
    fn from(tp: TrustProxy) -> Self {
        Self(tp)
    }
}

impl Forwarding {
    /// builds a Forwarding handler that trusts a list of strings that represent either specific IPs
    /// or a CIDR range.
    ///
    /// ```
    /// # use trillium_forwarding::Forwarding;
    /// let forwarding = Forwarding::trust_ips(["10.1.10.1"]);
    /// let forwarding = Forwarding::trust_ips(["10.1.10.1", "192.168.0.0/16"]);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if any of the provided strings is neither an ip address nor a CIDR range.
    pub fn trust_ips<'a>(ips: impl IntoIterator<Item = &'a str>) -> Self {
        Self(TrustProxy::Cidr(
            ips.into_iter()
                .map(|ip| {
                    ip.parse()
                        .unwrap_or_else(|_| panic!("could not parse `{ip}` as an ip or cidr range"))
                })
                .collect(),
        ))
    }

    /// builds a Forwarding handler that trusts a peer ip based on the provided predicate function.
    ///
    /// ```
    /// # use trillium_forwarding::Forwarding;
    /// # use std::net::IpAddr;
    /// let forwarding = Forwarding::trust_fn(IpAddr::is_loopback);
    /// let forwarding = Forwarding::trust_fn(|ip| match ip {
    ///     IpAddr::V6(_) => false,
    ///     IpAddr::V4(ipv4) => ipv4.is_link_local(),
    /// });
    /// ```
    pub fn trust_fn<F>(trust_predicate: F) -> Self
    where
        F: Fn(&IpAddr) -> bool + Send + Sync + 'static,
    {
        Self(TrustProxy::Function(TrustFn::from(trust_predicate)))
    }

    /// builds a Forwarding handler that expects that all http connections
    /// will always come from a trusted and spec-compliant reverse
    /// proxy. This should only be used in situations in which the
    /// application is either running inside of a vpc and the reverse
    /// proxy ip cannot be known. Using an overbroad trust rule such as
    /// `trust_always` introduces security risk to an application, as it
    /// allows any request to forge Forwarded headers.
    pub fn trust_always() -> Self {
        Self(TrustProxy::Always)
    }
}

impl Handler for Forwarding {
    async fn run(&self, mut conn: Conn) -> Conn {
        if !self.0.is_trusted(conn.peer_ip()) {
            return conn;
        }

        let forwarded = match Forwarded::from_headers(conn.request_headers()) {
            Ok(Some(forwarded)) => forwarded.into_owned(),
            Err(error) => {
                log::error!("{error}");
                return conn
                    .halt()
                    .with_state(error)
                    .with_status(Status::BadRequest);
            }
            Ok(None) => return conn,
        };

        log::debug!("received trusted forwarded {:?}", forwarded);

        let inner_mut: &mut trillium_http::Conn<Box<dyn Transport>> = conn.as_mut();

        if let Some(host) = forwarded.host() {
            inner_mut.set_host(String::from(host));
        }

        if let Some(proto) = forwarded.proto() {
            inner_mut.set_secure(proto.eq_ignore_ascii_case("https"));
        }

        let peer_ip = self
            .0
            .rightmost_untrusted(&forwarded.forwarded_for(), inner_mut.peer_ip());
        inner_mut.set_peer_ip(peer_ip);

        conn.with_state(forwarded)
    }
}
