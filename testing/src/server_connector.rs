use crate::{Runtime, TestTransport};
use async_channel::Receiver;
use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use trillium::{Handler, Transport};
use trillium_http::ServerConfig;
use trillium_server_common::Connector;
use url::Url;

/// a bridge between trillium servers and clients
#[derive(Debug, fieldwork::Fieldwork)]
pub struct ServerConnector<H> {
    /// the handler
    #[field(get, deref = false)]
    handler: Arc<H>,

    /// the runtime
    #[field(with, set, get, into)]
    runtime: Runtime,

    /// the server config
    #[field(with, set, get(deref = false), into)]
    server_config: Arc<ServerConfig>,

    pub(crate) client_peer_ips_receiver: Option<Receiver<IpAddr>>,
    pub(crate) server_peer_ips_receiver: Option<Receiver<IpAddr>>,
}

impl<H> Clone for ServerConnector<H> {
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
            runtime: self.runtime.clone(),
            server_config: self.server_config.clone(),
            client_peer_ips_receiver: self.client_peer_ips_receiver.clone(),
            server_peer_ips_receiver: self.server_peer_ips_receiver.clone(),
        }
    }
}

impl<H: Handler> ServerConnector<H> {
    /// builds a new ServerConnector
    pub fn new(handler: H) -> Self {
        Self {
            handler: Arc::new(handler),
            runtime: crate::runtime().into(),
            server_config: Arc::default(),
            client_peer_ips_receiver: None,
            server_peer_ips_receiver: None,
        }
    }

    /// opens a new connection to this virtual server, returning the client transport
    pub async fn connect(&self, secure: bool) -> TestTransport {
        let (mut client_transport, mut server_transport) = TestTransport::new();
        if let Some(server_ip) = self
            .client_peer_ips_receiver
            .as_ref()
            .and_then(|channel| channel.try_recv().ok())
        {
            client_transport.set_peer_ip(server_ip);
        }

        if let Some(client_ip) = self
            .server_peer_ips_receiver
            .as_ref()
            .and_then(|channel| channel.try_recv().ok())
        {
            server_transport.set_peer_ip(client_ip);
        }

        let handler = Arc::clone(&self.handler);
        let server_config = Arc::clone(&self.server_config);

        let peer_ip = server_transport
            .peer_addr()
            .ok()
            .flatten()
            .map(|addr| addr.ip());

        self.runtime.spawn(async move {
            server_config
                .run(server_transport, |mut conn| {
                    let handler = Arc::clone(&handler);
                    async move {
                        conn.set_peer_ip(peer_ip).set_secure(secure);
                        let conn = handler.run(conn.into()).await;
                        let conn = handler.before_send(conn).await;
                        let mut inner = conn.into_inner::<TestTransport>();
                        let state = std::mem::take(inner.state_mut());
                        *inner.transport().state().write().unwrap() = state;
                        inner
                    }
                })
                .await
                .unwrap();
        });

        client_transport
    }
}

impl<H: Handler> Connector for ServerConnector<H> {
    type Runtime = Runtime;
    type Transport = TestTransport;
    type Udp = ();

    async fn connect(&self, url: &Url) -> io::Result<Self::Transport> {
        Ok(self.connect(url.scheme() == "https").await)
    }

    fn runtime(&self) -> Self::Runtime {
        #[allow(clippy::clone_on_copy)]
        self.runtime.clone()
    }

    async fn resolve(&self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(vec![SocketAddr::from(([0, 0, 0, 0], 0))])
    }
}

/// build a connector from this handler
pub fn connector(handler: impl Handler) -> impl Connector {
    ServerConnector::new(handler)
}
