use super::invalid;
use crate::{Url, url};
use ipnet::IpNet;
use std::{io, net::IpAddr};

/// A proxy's bypass entries, in the syntax [`Proxy::except`](super::Proxy::except) documents.
#[derive(Clone, Debug, Default)]
pub(crate) struct NoProxy {
    entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
struct Entry {
    host: Host,
    port: Option<u16>,
}

#[derive(Clone, Debug)]
enum Host {
    Domain(String),
    Ip(IpAddr),
    Network(IpNet),
}

impl NoProxy {
    /// Parse a `no_proxy` environment value: comma-separated entries, with whitespace around
    /// them and empty entries ignored.
    pub(crate) fn new(value: &str) -> io::Result<Self> {
        let mut no_proxy = Self::default();
        for entry in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            no_proxy.push(entry)?;
        }
        Ok(no_proxy)
    }

    /// Add one entry, written exactly: no surrounding whitespace, and not a list.
    pub(crate) fn push(&mut self, entry: &str) -> io::Result<()> {
        let entry = Entry::parse(entry)
            .map_err(|problem| invalid(&format!("proxy bypass entry {entry:?}: {problem}")))?;
        self.entries.push(entry);
        Ok(())
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.entries.extend(other.entries);
    }

    /// Whether this URL's host and effective port match a bypass entry.
    pub(crate) fn contains(&self, url: &Url) -> bool {
        let Some(host) = url.host() else {
            return false;
        };
        let port = url.port_or_known_default();
        self.entries.iter().any(|entry| entry.matches(&host, port))
    }
}

impl Entry {
    fn matches(&self, host: &url::Host<&str>, port: Option<u16>) -> bool {
        if self.port.is_some() && self.port != port {
            return false;
        }
        match (&self.host, host) {
            (Host::Domain(pattern), url::Host::Domain(domain)) => {
                let domain = domain.trim_end_matches('.');
                domain == pattern
                    || domain
                        .strip_suffix(pattern.as_str())
                        .is_some_and(|prefix| prefix.ends_with('.'))
            }
            (Host::Ip(ip), url::Host::Ipv4(value)) => *ip == IpAddr::V4(*value),
            (Host::Ip(ip), url::Host::Ipv6(value)) => *ip == IpAddr::V6(*value),
            (Host::Network(network), url::Host::Ipv4(value)) => {
                network.contains(&IpAddr::from(*value))
            }
            (Host::Network(network), url::Host::Ipv6(value)) => {
                network.contains(&IpAddr::from(*value))
            }
            _ => false,
        }
    }

    fn parse(value: &str) -> io::Result<Self> {
        if value.is_empty() {
            return Err(invalid("empty"));
        }
        if value.trim() != value {
            return Err(invalid("surrounding whitespace"));
        }
        if value.contains(',') {
            return Err(invalid("contains a comma; pass each entry separately"));
        }
        if value == "*" {
            return Err(invalid(
                "matches every host, so the proxy would carry nothing",
            ));
        }
        if let Ok(ip) = value.parse() {
            return Ok(Self {
                host: Host::Ip(ip),
                port: None,
            });
        }
        if let Ok(network) = value.parse() {
            return Ok(Self {
                host: Host::Network(network),
                port: None,
            });
        }
        let (host, port) = if let Some(rest) = value.strip_prefix('[') {
            let (host, suffix) = rest
                .split_once(']')
                .ok_or_else(|| invalid("unclosed IPv6 literal"))?;
            let ip: std::net::Ipv6Addr =
                host.parse().map_err(|_| invalid("invalid IPv6 literal"))?;
            let port = if suffix.is_empty() {
                None
            } else {
                Some(parse_port(
                    suffix
                        .strip_prefix(':')
                        .ok_or_else(|| invalid("invalid IPv6 port"))?,
                )?)
            };
            return Ok(Self {
                host: Host::Ip(ip.into()),
                port,
            });
        } else if let Some((host, port)) = value.rsplit_once(':') {
            (host, Some(parse_port(port)?))
        } else {
            (value, None)
        };
        if let Ok(ip) = host.parse() {
            return Ok(Self {
                host: Host::Ip(ip),
                port,
            });
        }
        if let Ok(network) = host.parse() {
            return Ok(Self {
                host: Host::Network(network),
                port,
            });
        }
        let domain = host
            .strip_prefix("*.")
            .or_else(|| host.strip_prefix('.'))
            .unwrap_or(host)
            .trim_end_matches('.');
        if domain.is_empty() || domain.contains(['*', '/', '@', '?', '#', ':', '[', ']']) {
            return Err(invalid("invalid domain"));
        }
        let host = url::Host::parse(domain).map_err(|_| invalid("invalid domain"))?;
        let url::Host::Domain(domain) = host else {
            return Err(invalid("invalid domain"));
        };
        Ok(Self {
            host: Host::Domain(domain),
            port,
        })
    }
}

fn parse_port(port: &str) -> io::Result<u16> {
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("invalid port"));
    }
    port.parse().map_err(|_| invalid("invalid port"))
}
