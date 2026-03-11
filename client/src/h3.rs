use crate::{Pool, h3::alt_svc::DEFAULT_BROKEN_DURATION, pool::PoolEntry};
use alt_svc::AltSvcCache;
use std::time::Duration;
use trillium_server_common::{
    ArcedQuicConnector, QuicConnection,
    url::{Origin, Url},
};

mod alt_svc;

/// Shared state for HTTP/3 support on a [`Client`].
///
/// Present only when the client has been configured with a [`QuicConnector`] via
/// [`Client::new_with_quic`]. All fields are cheaply cloneable (Arc-backed).
#[derive(Clone, Debug)]
pub(crate) struct H3ClientState {
    pub(crate) connector: ArcedQuicConnector,
    pub(crate) pool: Pool<Origin, QuicConnection>,
    pub(crate) alt_svc: AltSvcCache,
    pub(crate) broken_duration: Duration,
}

impl H3ClientState {
    pub(crate) fn update_alt_svc(&self, alt_svc: &str, url: &Url) {
        self.alt_svc.update(alt_svc, url);
    }

    pub(crate) fn mark_broken(&self, origin: &Origin) {
        if let Some(mut entry) = self.alt_svc.get_mut(origin) {
            entry.mark_broken(self.broken_duration);
        }
    }

    pub(crate) async fn get_or_create_quic_conn(
        &self,
        origin: &Origin,
        host: &str,
        port: u16,
    ) -> Result<QuicConnection, std::io::Error> {
        if let Some(conn) = self.pool.peek_candidate(origin) {
            return Ok(conn);
        }
        let conn = self.connector.connect(host, port).await?;
        self.pool
            .insert(origin.clone(), PoolEntry::new(conn.clone(), None));
        Ok(conn)
    }

    pub(crate) fn new(connector: ArcedQuicConnector) -> Self {
        Self {
            connector,
            pool: Default::default(),
            alt_svc: AltSvcCache::default(),
            broken_duration: DEFAULT_BROKEN_DURATION,
        }
    }
}
