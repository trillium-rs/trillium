use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use trillium_http::Synthetic;
use trillium_macros::{AsyncRead, AsyncWrite, Transport};
use trillium_server_common::{AsyncRead, AsyncWrite, Transport};

#[derive(Debug)]
struct CallStats(Arc<Mutex<HashMap<&'static str, usize>>>);

impl CallStats {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    fn inc(&self, name: &'static str) {
        *self.0.lock().unwrap().entry(name).or_insert(0) += 1;
    }

    fn get(&self, name: &'static str) -> usize {
        *self.0.lock().unwrap().get(name).unwrap_or(&0)
    }
}

#[derive(Debug, AsyncRead, AsyncWrite)]
struct InnerTransport {
    #[async_io]
    inner: Synthetic,
    stats: CallStats,
}

impl InnerTransport {
    fn new() -> Self {
        Self {
            inner: Synthetic::from(()),
            stats: CallStats::new(),
        }
    }
}

impl Transport for InnerTransport {
    fn set_linger(&mut self, _linger: Option<std::time::Duration>) -> std::io::Result<()> {
        self.stats.inc("set_linger");
        Ok(())
    }

    fn set_nodelay(&mut self, _nodelay: bool) -> std::io::Result<()> {
        self.stats.inc("set_nodelay");
        Ok(())
    }

    fn set_ip_ttl(&mut self, _ttl: u32) -> std::io::Result<()> {
        self.stats.inc("set_ip_ttl");
        Ok(())
    }

    fn peer_addr(&self) -> std::io::Result<Option<std::net::SocketAddr>> {
        self.stats.inc("peer_addr");
        Ok(None)
    }
}

fn call_all_once(t: &mut impl Transport) {
    t.set_linger(None).unwrap();
    t.set_nodelay(true).unwrap();
    t.set_ip_ttl(0).unwrap();
    t.peer_addr().unwrap();
}

macro_rules! define_transport_with_except {
    ($t:ident $(, except = $e:tt)? $(,)?) => {
        #[derive(Debug, AsyncRead, AsyncWrite, Transport)]
        struct $t {
            #[transport$((except = $e))?] #[async_io]
            inner: InnerTransport,
            stats: CallStats,
        }
        #[allow(unused)]
        impl $t {
            fn new() -> Self {
                Self {
                    inner: InnerTransport::new(),
                    stats: CallStats::new(),
                }
            }

            fn set_linger(&mut self, _linger: Option<std::time::Duration>) -> std::io::Result<()> {
                self.stats.inc("set_linger");
                Ok(())
            }

            fn set_nodelay(&mut self, _nodelay: bool) -> std::io::Result<()> {
                self.stats.inc("set_nodelay");
                Ok(())
            }

            fn set_ip_ttl(&mut self, _ttl: u32) -> std::io::Result<()> {
                self.stats.inc("set_ip_ttl");
                Ok(())
            }

            fn peer_addr(&self) -> std::io::Result<Option<std::net::SocketAddr>> {
                self.stats.inc("peer_addr");
                Ok(None)
            }
        }
    }
}

#[test]
fn full_derive() {
    define_transport_with_except!(OuterTransport);
    let mut outer = OuterTransport::new();
    call_all_once(&mut outer);
    assert_eq!(outer.stats.get("set_linger"), 0);
    assert_eq!(outer.stats.get("set_nodelay"), 0);
    assert_eq!(outer.stats.get("set_ip_ttl"), 0);
    assert_eq!(outer.stats.get("peer_addr"), 0);
    assert_eq!(outer.inner.stats.get("set_linger"), 1);
    assert_eq!(outer.inner.stats.get("set_nodelay"), 1);
    assert_eq!(outer.inner.stats.get("set_ip_ttl"), 1);
    assert_eq!(outer.inner.stats.get("peer_addr"), 1);
}

#[test]
fn override_one() {
    define_transport_with_except!(OuterTransport, except = set_linger);
    let mut outer = OuterTransport::new();
    call_all_once(&mut outer);
    assert_eq!(outer.stats.get("set_linger"), 1);
    assert_eq!(outer.stats.get("set_nodelay"), 0);
    assert_eq!(outer.stats.get("set_ip_ttl"), 0);
    assert_eq!(outer.stats.get("peer_addr"), 0);
    assert_eq!(outer.inner.stats.get("set_linger"), 0);
    assert_eq!(outer.inner.stats.get("set_nodelay"), 1);
    assert_eq!(outer.inner.stats.get("set_ip_ttl"), 1);
    assert_eq!(outer.inner.stats.get("peer_addr"), 1);
}

#[test]
fn override_two() {
    define_transport_with_except!(OuterTransport, except = [set_nodelay, peer_addr]);
    let mut outer = OuterTransport::new();
    call_all_once(&mut outer);
    assert_eq!(outer.stats.get("set_linger"), 0);
    assert_eq!(outer.stats.get("set_nodelay"), 1);
    assert_eq!(outer.stats.get("set_ip_ttl"), 0);
    assert_eq!(outer.stats.get("peer_addr"), 1);
    assert_eq!(outer.inner.stats.get("set_linger"), 1);
    assert_eq!(outer.inner.stats.get("set_nodelay"), 0);
    assert_eq!(outer.inner.stats.get("set_ip_ttl"), 1);
    assert_eq!(outer.inner.stats.get("peer_addr"), 0);
}

#[test]
fn override_all() {
    define_transport_with_except!(
        OuterTransport,
        except = [set_linger, set_nodelay, set_ip_ttl, peer_addr],
    );
    let mut outer = OuterTransport::new();
    call_all_once(&mut outer);
    assert_eq!(outer.stats.get("set_linger"), 1);
    assert_eq!(outer.stats.get("set_nodelay"), 1);
    assert_eq!(outer.stats.get("set_ip_ttl"), 1);
    assert_eq!(outer.stats.get("peer_addr"), 1);
    assert_eq!(outer.inner.stats.get("set_linger"), 0);
    assert_eq!(outer.inner.stats.get("set_nodelay"), 0);
    assert_eq!(outer.inner.stats.get("set_ip_ttl"), 0);
    assert_eq!(outer.inner.stats.get("peer_addr"), 0);
}
