//! Integration test for DNS-over-HTTPS resolution.
//!
//! Stands up a mock DoH resolver and a target server, both on real loopback
//! ports, and drives a `with_doh`-configured client through the full loop:
//! issue the DoH query over the client's own pool, parse the response, and
//! connect to the resolved address — for a hostname that has no real DNS entry,
//! so the connection can only succeed if DoH did the resolving.
#![cfg(feature = "hickory")]

use hickory_proto::{
    op::{Message, MessageType, OpCode},
    rr::{Name, RData, Record, rdata::A},
};
use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use trillium::Conn;
use trillium_client::Client;
use trillium_testing::{harness, test};

/// A wire-format DoH response resolving any queried name to `ip` via an A record.
fn a_record_response(ip: Ipv4Addr) -> Vec<u8> {
    let mut message = Message::new(0, MessageType::Response, OpCode::Query);
    message.add_answer(Record::from_rdata(
        Name::from_utf8("resolved.test.").unwrap(),
        60,
        RData::A(A(ip)),
    ));
    message.to_vec().unwrap()
}

#[test(harness)]
async fn resolves_and_connects_through_doh() {
    let target = trillium_smol::config()
        .with_host("127.0.0.1")
        .with_port(0)
        .spawn(|conn: Conn| async move { conn.ok("hello from target") });
    let target_addr = target.info().await.tcp_socket_addr().copied().unwrap();
    let target_ip = match target_addr.ip() {
        std::net::IpAddr::V4(ip) => ip,
        other => panic!("expected an ipv4 loopback bind, got {other}"),
    };

    let queries = Arc::new(AtomicUsize::new(0));
    let resolver_queries = queries.clone();
    let resolver = trillium_smol::config()
        .with_host("127.0.0.1")
        .with_port(0)
        .spawn(move |conn: Conn| {
            let queries = resolver_queries.clone();
            async move {
                queries.fetch_add(1, Ordering::SeqCst);
                conn.with_response_header("content-type", "application/dns-message")
                    .ok(a_record_response(target_ip))
            }
        });
    let resolver_addr = resolver.info().await.tcp_socket_addr().copied().unwrap();

    let client = Client::new(trillium_smol::ClientConfig::default())
        .with_doh(format!("http://{resolver_addr}/dns-query"));

    // `nonexistent.test` has no real DNS entry — reaching the target proves the
    // client resolved it through the mock resolver, not the system resolver.
    let url = format!("http://nonexistent.test:{}/", target_addr.port());

    let mut conn = client.get(url.as_str()).await.unwrap();
    assert_eq!(
        conn.response_body().read_string().await.unwrap(),
        "hello from target"
    );
    assert!(
        queries.load(Ordering::SeqCst) >= 1,
        "the resolver should have been queried at least once"
    );

    // A second request to the same host is served from the DNS cache: no further
    // queries reach the resolver.
    let after_first = queries.load(Ordering::SeqCst);
    let mut conn = client.get(url.as_str()).await.unwrap();
    assert_eq!(
        conn.response_body().read_string().await.unwrap(),
        "hello from target"
    );
    assert_eq!(
        queries.load(Ordering::SeqCst),
        after_first,
        "the second request should be served from the DNS cache"
    );

    target.shut_down().await;
    resolver.shut_down().await;
}
