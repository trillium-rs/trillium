use super::{ArcedConnector, Connector, Destination};
use crate::Url;
use futures_lite::future::block_on;
use std::{borrow::Cow, io, net::SocketAddr};
use trillium_http::Synthetic;

struct LegacyConnector;

impl Connector for LegacyConnector {
    type Runtime = crate::Runtime;
    type Transport = Synthetic;
    type Udp = ();

    async fn connect(&self, _: &Url) -> io::Result<Self::Transport> {
        panic!("connector fixture does not dial")
    }

    async fn connect_to(&self, _: Destination) -> io::Result<Self::Transport> {
        panic!("connector fixture does not dial")
    }

    async fn resolve(&self, _: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
        panic!("connector fixture does not resolve")
    }

    fn runtime(&self) -> Self::Runtime {
        panic!("connector fixture does not run tasks")
    }
}

#[test]
fn default_connect_over_passes_non_secure_and_rejects_secure() {
    use futures_lite::{AsyncReadExt, AsyncWriteExt};
    use trillium_testing::TestTransport;

    block_on(async {
        let (client, mut server) = TestTransport::new();
        let mut transport = LegacyConnector
            .connect_over(
                Box::new(client),
                Destination::new_with_host(false, "origin.example", 80),
            )
            .await
            .unwrap();

        transport.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");

        server.write_all(b"pong");
        transport.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");

        let (client, _server) = TestTransport::new();
        let error = LegacyConnector
            .connect_over(
                Box::new(client),
                Destination::new_with_host(true, "origin.example", 443),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);

        let (client, _server) = TestTransport::new();
        let error = ArcedConnector::new(LegacyConnector)
            .connect_over(
                Box::new(client),
                Destination::new_with_host(true, "origin.example", 443),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    });
}

struct ConnectOverConnector;

impl Connector for ConnectOverConnector {
    type Runtime = crate::Runtime;
    type Transport = Synthetic;
    type Udp = ();

    async fn connect(&self, url: &Url) -> io::Result<Self::Transport> {
        LegacyConnector.connect(url).await
    }

    async fn connect_over(
        &self,
        mut transport: Box<dyn crate::Transport>,
        destination: Destination,
    ) -> io::Result<Box<dyn crate::Transport>> {
        use futures_lite::AsyncWriteExt;

        assert!(destination.secure());
        assert_eq!(destination.host(), Some("origin.example"));
        assert_eq!(destination.port(), 443);
        assert_eq!(
            destination.alpn().unwrap(),
            [Cow::Borrowed(b"h2".as_slice())]
        );
        transport.write_all(b"layered").await?;
        Ok(transport)
    }

    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        LegacyConnector.resolve(host, port).await
    }

    fn runtime(&self) -> Self::Runtime {
        panic!("connector fixture does not run tasks")
    }
}

#[test]
fn arced_connector_forwards_connect_over() {
    use futures_lite::AsyncReadExt;
    use trillium_testing::TestTransport;

    block_on(async {
        let (client, mut server) = TestTransport::new();
        let connector = ArcedConnector::new(ConnectOverConnector);
        let _transport = connector
            .connect_over(
                Box::new(client),
                Destination::new_with_host(true, "origin.example", 443)
                    .with_alpn([Cow::Borrowed(b"h2".as_slice())]),
            )
            .await
            .unwrap();

        let mut buf = [0u8; 7];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"layered");
    });
}
