use crate::bytes;
use full_duplex_async_copy::full_duplex_copy;
use std::fmt::Debug;
use trillium::{Conn, Handler, Upgrade};
use trillium_client::{ArcedConnector, Connector};
use trillium_http::{Method, Status};
use url::Url;

#[derive(Debug)]
/// trillium handler to implement Connect proxying
pub struct ForwardProxyConnect(ArcedConnector);

#[derive(Debug)]
struct ForwardUpgrade(trillium_http::transport::BoxedTransport);

impl ForwardProxyConnect {
    /// construct a new ForwardProxyConnect
    pub fn new(connector: impl Connector) -> Self {
        Self(ArcedConnector::new(connector))
    }
}

impl Handler for ForwardProxyConnect {
    async fn run(&self, conn: Conn) -> Conn {
        if conn.method() == Method::Connect {
            let Ok(url) = Url::parse(&format!("http://{}", conn.path())) else {
                return conn.with_status(Status::BadRequest).halt();
            };

            if url.cannot_be_a_base() {
                return conn.with_status(Status::BadRequest).halt();
            }

            let Ok(tcp) = Connector::connect(&self.0, &url).await else {
                return conn.with_status(Status::BadGateway).halt();
            };

            conn.with_status(Status::Ok)
                .with_state(ForwardUpgrade(tcp))
                .halt()
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state.contains::<ForwardUpgrade>()
    }

    async fn upgrade(&self, mut upgrade: Upgrade) {
        let Some(ForwardUpgrade(upstream)) = upgrade.state.take() else {
            return;
        };
        let downstream = upgrade;
        match full_duplex_copy(upstream, downstream).await {
            Err(e) => log::error!("upgrade stream error: {:?}", e),
            Ok((up, down)) => {
                log::debug!("streamed upgrade {} up and {} down", bytes(up), bytes(down))
            }
        }
    }
}
