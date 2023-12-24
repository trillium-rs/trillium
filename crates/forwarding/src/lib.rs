/*!
# Trillium handler for `x-forwarded-*` / `forwarded`

This simple handler rewrites the request's host, secure setting, and
peer ip based on headers added by a trusted reverse proxy.

The specific headers that are understood by this handler are:

* [`Forwarded`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded)
* or some combination of the following
    - [`X-Forwarded-For`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-For)
    - [`X-Forwarded-Proto`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-Proto)
    - [`X-Forwarded-Host`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/X-Forwarded-Host)

There are several ways of specifying when to trust a peer ip address,
and the narrowest possible trust rules should be used for a given
deployment so as to decrease the chance for a threat actor to generate
a request with forwarded headers that we mistakenly trust.
*/
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
mod forwarded;
pub use forwarded::Forwarded;

mod parse_utils;

use std::{fmt::Debug, net::IpAddr, ops::Deref};
use trillium::{async_trait, Conn, Handler, Status};

#[derive(Debug)]
#[non_exhaustive]
enum TrustProxy {
    Always,
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
        f.debug_tuple("TrustPredicate").field(&"..").finish()
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
}

/**
Trillium handler for `forwarded`/`x-forwarded-*` headers

See crate-level docs for an explanation
*/
#[derive(Default, Debug)]
pub struct Forwarding(TrustProxy);

impl From<TrustProxy> for Forwarding {
    fn from(tp: TrustProxy) -> Self {
        Self(tp)
    }
}

impl Forwarding {
    /**
    builds a Forwarding handler that trusts a list of strings that represent either specific IPs or a CIDR range.
    ```
    # use trillium_forwarding::Forwarding;
    let forwarding = Forwarding::trust_ips(["10.1.10.1"]);
    let forwarding = Forwarding::trust_ips(["10.1.10.1", "192.168.0.0/16"]);
    ```
    */
    pub fn trust_ips<'a>(ips: impl IntoIterator<Item = &'a str>) -> Self {
        Self(TrustProxy::Cidr(
            ips.into_iter().map(|ip| ip.parse().unwrap()).collect(),
        ))
    }

    /**
    builds a Forwarding handler that trusts a peer ip based on the provided predicate function.

    ```
    # use trillium_forwarding::Forwarding;
    # use std::net::IpAddr;
    let forwarding = Forwarding::trust_fn(IpAddr::is_loopback);
    let forwarding = Forwarding::trust_fn(|ip| match ip {
        IpAddr::V6(_) => false,
        IpAddr::V4(ipv4) => ipv4.is_link_local()
    });
    ```
     */
    pub fn trust_fn<F>(trust_predicate: F) -> Self
    where
        F: Fn(&IpAddr) -> bool + Send + Sync + 'static,
    {
        Self(TrustProxy::Function(TrustFn::from(trust_predicate)))
    }

    /**
    builds a Forwarding handler that expects that all http connections
    will always come from a trusted and spec-compliant reverse
    proxy. This should only be used in situations in which the
    application is either running inside of a vpc and the reverse
    proxy ip cannot be known. Using an overbroad trust rule such as
    `trust_always` introduces security risk to an application, as it
    allows any request to forge Forwarded headers.

    */
    pub fn trust_always() -> Self {
        Self(TrustProxy::Always)
    }
}

impl Default for TrustProxy {
    fn default() -> Self {
        Self::Never
    }
}

#[async_trait]
impl Handler for Forwarding {
    async fn run(&self, mut conn: Conn) -> Conn {
        if !self.0.is_trusted(conn.inner().peer_ip()) {
            return conn;
        }

        let forwarded = match Forwarded::from_headers(conn.headers()) {
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

        log::debug!("received trusted forwarded {:?}", &forwarded);

        let inner_mut = conn.inner_mut();

        if let Some(host) = forwarded.host() {
            inner_mut.set_host(String::from(host));
        }

        if let Some(proto) = forwarded.proto() {
            inner_mut.set_secure(proto == "https");
        }

        if let Some(ip) = forwarded.forwarded_for().first() {
            if let Ok(ip_addr) = ip.trim_start_matches('[').trim_end_matches(']').parse() {
                inner_mut.set_peer_ip(Some(ip_addr));
            }
        }

        conn.with_state(forwarded)
    }
}
