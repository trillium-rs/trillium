use std::net::IpAddr;

/// Normalize an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) to the IPv4 address it denotes.
///
/// A dual-stack listener — one bound to `::`, which is how a single socket serves both families —
/// reports every IPv4 peer in the mapped form. That is an artifact of the socket, not a fact about
/// the client: the peer is an IPv4 host, and nothing downstream should have to know the difference.
///
/// Left mapped, it silently breaks anything that reasons about the address. Address-family checks
/// take a v4 peer for a v6 one. CIDR matches miss. IPv6 prefix masking is actively dangerous: a
/// mapped address carries its 32 distinguishing bits *below* a `/64`, so masking to a `/64` — the
/// usual way to treat one client's IPv6 allocation as one network — zeroes precisely what
/// distinguishes clients, collapsing the entire IPv4 internet onto a single key. And a log line
/// bearing `::ffff:203.0.113.9` does not match a firewall rule written for `203.0.113.9`, so
/// whatever reads those logs to act on them acts on nothing.
///
/// Applied at each point a peer address enters trillium — the accept loop for TCP, the connection
/// for QUIC — so that nothing further in has to care.
pub(crate) fn unmap_ipv4(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

#[cfg(test)]
mod tests {
    use super::unmap_ipv4;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn unmaps_ipv4_mapped_addresses() {
        assert_eq!(
            unmap_ipv4("::ffff:203.0.113.9".parse().unwrap()),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9))
        );
    }

    #[test]
    fn leaves_native_addresses_alone() {
        let v4 = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
        assert_eq!(unmap_ipv4(v4), v4);

        let v6 = IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(unmap_ipv4(v6), v6);

        // `::1` is loopback, not a mapped address, and must not be mistaken for `0.0.0.1`.
        let loopback = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(unmap_ipv4(loopback), loopback);
    }
}
