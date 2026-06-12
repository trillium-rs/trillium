//! Wire codec: building DNS query messages and parsing responses with `hickory-proto`, plus the
//! [`Resolved`] / [`ServiceBinding`] types they produce. Transport-independent — every
//! [`DnsTransport`](super::DnsTransport) shares this.

use super::MAX_TTL;
use hickory_proto::{
    op::{Message, MessageType, OpCode, Query},
    rr::{
        Name, RData, RecordType,
        rdata::svcb::{SVCB, SvcParamValue},
    },
};
use smallvec::SmallVec;
use std::{
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

/// A resolved host: address records plus any service bindings (SVCB/HTTPS
/// records) the resolver returned, in ascending priority order.
///
/// Addresses are stored without a port — the port is request-specific and
/// applied by the caller — so a single resolution serves every port and
/// protocol for the host.
#[derive(Debug, Clone, Default)]
pub(crate) struct Resolved {
    pub(crate) addrs: Vec<IpAddr>,
    pub(crate) services: Vec<ServiceBinding>,
}

/// One ServiceMode SVCB/HTTPS record. AliasMode records (priority 0) are not
/// represented here; they're followed during query construction instead.
#[derive(Debug, Clone)]
pub(crate) struct ServiceBinding {
    pub(crate) priority: u16,
    /// The connect target, or `None` when the record's `TargetName` is `.`
    /// (meaning "the queried name itself").
    pub(crate) target: Option<String>,
    pub(crate) alpn: Vec<String>,
    pub(crate) port: Option<u16>,
    pub(crate) ipv4hint: Vec<Ipv4Addr>,
    pub(crate) ipv6hint: Vec<Ipv6Addr>,
}

impl ServiceBinding {
    /// Whether this binding advertises HTTP/3 (`h3` ALPN).
    pub(crate) fn advertises_h3(&self) -> bool {
        self.alpn.iter().any(|id| id == "h3")
    }

    /// The address hints carried by this binding, of both families.
    fn hint_addrs(&self) -> impl Iterator<Item = IpAddr> + '_ {
        self.ipv4hint
            .iter()
            .copied()
            .map(IpAddr::V4)
            .chain(self.ipv6hint.iter().copied().map(IpAddr::V6))
    }
}

impl Resolved {
    /// Pair every resolved A/AAAA address with `port`. Falls back to SVCB address hints when no
    /// A/AAAA records were returned, pairing each binding's hints with the binding's own `port`
    /// SvcParam (falling back to `port` when it specifies none).
    pub(crate) fn socket_addrs(&self, port: u16) -> SmallVec<[SocketAddr; 4]> {
        if self.addrs.is_empty() {
            self.services
                .iter()
                .flat_map(|binding| {
                    let binding_port = binding.port.unwrap_or(port);
                    binding
                        .hint_addrs()
                        .map(move |ip| SocketAddr::new(ip, binding_port))
                })
                .collect()
        } else {
            self.addrs
                .iter()
                .map(|&ip| SocketAddr::new(ip, port))
                .collect()
        }
    }

    /// Whether this resolution yields any connectable address (A/AAAA record or
    /// an SVCB hint).
    pub(super) fn has_addrs(&self) -> bool {
        !self.addrs.is_empty()
            || self
                .services
                .iter()
                .any(|s| s.hint_addrs().next().is_some())
    }

    pub(super) fn merge(&mut self, other: Resolved) {
        self.addrs.extend(other.addrs);
        self.services.extend(other.services);
    }
}

/// The DNS name to query for the HTTPS record of `host:port`.
///
/// For the default port (443) the HTTPS record lives at the host name itself;
/// other ports use the `_<port>._https` attrleaf prefix.
fn https_query_name(host: &str, port: u16) -> io::Result<Name> {
    let name = if port == 443 {
        host.to_string()
    } else {
        format!("_{port}._https.{host}")
    };
    Name::from_utf8(name).map_err(|e| io::Error::new(ErrorKind::InvalidInput, e))
}

/// Build a wire-format DNS query for `record_type` over `host:port`.
///
/// `HTTPS` queries use the host name (or `_<port>._https` attrleaf prefix);
/// address queries (`A`/`AAAA`) use the host name directly. The message ID is
/// fixed at 0 to maximize HTTP cache friendliness.
pub(crate) fn build_query(host: &str, port: u16, record_type: RecordType) -> io::Result<Vec<u8>> {
    let name = match record_type {
        RecordType::HTTPS => https_query_name(host, port)?,
        _ => Name::from_utf8(host).map_err(|e| io::Error::new(ErrorKind::InvalidInput, e))?,
    };

    let mut message = Message::new(0, MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.add_query(Query::query(name, record_type));
    message
        .to_vec()
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

/// Parse a wire-format DNS response into the addresses and service bindings it
/// carries, along with the minimum record TTL (used as the cache lifetime).
pub(crate) fn parse_response(bytes: &[u8]) -> io::Result<(Resolved, Duration)> {
    let message =
        Message::from_vec(bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;

    let mut resolved = Resolved::default();
    let mut min_ttl = u32::MAX;

    for record in message.all_sections() {
        match &record.data {
            RData::A(a) => resolved.addrs.push(IpAddr::V4(a.0)),
            RData::AAAA(aaaa) => resolved.addrs.push(IpAddr::V6(aaaa.0)),
            RData::HTTPS(https) => match service_binding(https) {
                Some(binding) => resolved.services.push(binding),
                None => continue,
            },
            _ => continue,
        }
        min_ttl = min_ttl.min(record.ttl);
    }

    resolved.services.sort_by_key(|s| s.priority);
    let ttl = Duration::from_secs(u64::from(min_ttl.min(MAX_TTL.as_secs() as u32)));
    Ok((resolved, ttl))
}

/// Project a hickory `SVCB` record into our [`ServiceBinding`]. Returns `None`
/// for AliasMode records (priority 0), which carry no service parameters.
fn service_binding(svcb: &SVCB) -> Option<ServiceBinding> {
    if svcb.svc_priority == 0 {
        return None;
    }

    let target = if svcb.target_name.is_root() {
        None
    } else {
        Some(svcb.target_name.to_utf8())
    };

    let mut binding = ServiceBinding {
        priority: svcb.svc_priority,
        target,
        alpn: Vec::new(),
        port: None,
        ipv4hint: Vec::new(),
        ipv6hint: Vec::new(),
    };

    for (_key, value) in &svcb.svc_params {
        match value {
            SvcParamValue::Alpn(alpn) => binding.alpn = alpn.0.clone(),
            SvcParamValue::Port(port) => binding.port = Some(*port),
            SvcParamValue::Ipv4Hint(hint) => {
                binding.ipv4hint = hint.0.iter().map(|a| a.0).collect();
            }
            SvcParamValue::Ipv6Hint(hint) => {
                binding.ipv6hint = hint.0.iter().map(|a| a.0).collect();
            }
            _ => {}
        }
    }

    Some(binding)
}

#[cfg(test)]
pub(super) fn sample_response() -> Vec<u8> {
    use hickory_proto::rr::{
        Record,
        rdata::{
            A, AAAA, HTTPS,
            svcb::{Alpn, IpHint, SvcParamKey},
        },
    };

    let svcb = SVCB::new(
        1,
        Name::from_utf8("svc.example.net.").unwrap(),
        vec![
            (
                SvcParamKey::Alpn,
                SvcParamValue::Alpn(Alpn(vec!["h3".into(), "h2".into()])),
            ),
            (SvcParamKey::Port, SvcParamValue::Port(8443)),
            (
                SvcParamKey::Ipv4Hint,
                SvcParamValue::Ipv4Hint(IpHint(vec![A(Ipv4Addr::new(192, 0, 2, 1))])),
            ),
        ],
    );

    let mut message = Message::new(0, MessageType::Response, OpCode::Query);
    let name = Name::from_utf8("example.com.").unwrap();
    message.add_answer(Record::from_rdata(
        name.clone(),
        300,
        RData::A(A(Ipv4Addr::new(192, 0, 2, 9))),
    ));
    message.add_answer(Record::from_rdata(
        name.clone(),
        300,
        RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)),
    ));
    message.add_answer(Record::from_rdata(name, 120, RData::HTTPS(HTTPS(svcb))));
    message.to_vec().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_round_trips() {
        let bytes = build_query("example.com", 443, RecordType::HTTPS).unwrap();
        let message = Message::from_vec(&bytes).unwrap();
        let query = &message.queries[0];
        assert_eq!(query.query_type(), RecordType::HTTPS);
        assert_eq!(query.name().to_utf8(), "example.com.");
    }

    #[test]
    fn non_default_port_uses_attrleaf_prefix() {
        let bytes = build_query("example.com", 8443, RecordType::HTTPS).unwrap();
        let message = Message::from_vec(&bytes).unwrap();
        assert_eq!(
            message.queries[0].name().to_utf8(),
            "_8443._https.example.com."
        );
    }

    #[test]
    fn address_query_uses_plain_name() {
        let bytes = build_query("example.com", 8443, RecordType::A).unwrap();
        let message = Message::from_vec(&bytes).unwrap();
        assert_eq!(message.queries[0].query_type(), RecordType::A);
        assert_eq!(message.queries[0].name().to_utf8(), "example.com.");
    }

    #[test]
    fn parses_addrs_and_service_binding() {
        let (resolved, ttl) = parse_response(&sample_response()).unwrap();

        assert_eq!(resolved.addrs.len(), 2);
        assert_eq!(resolved.services.len(), 1);
        assert_eq!(ttl, Duration::from_secs(120)); // minimum across records

        let binding = &resolved.services[0];
        assert!(binding.advertises_h3());
        assert_eq!(binding.target.as_deref(), Some("svc.example.net."));
        assert_eq!(binding.port, Some(8443));
        assert_eq!(binding.ipv4hint, vec![Ipv4Addr::new(192, 0, 2, 1)]);
    }
}
