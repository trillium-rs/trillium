use futures_lite::{AsyncRead, AsyncWrite};
use quinn::VarInt;
use std::{io, net::SocketAddr};
use trillium_server_common::{QuicConnection, Transport};

/// A bidirectional QUIC stream, combining quinn's split send/recv
/// into a single [`Transport`].
#[derive(trillium_macros::AsyncRead, trillium_macros::AsyncWrite)]
pub struct QuinnTransport {
    #[async_read]
    recv: async_compat::Compat<quinn::RecvStream>,
    #[async_write]
    send: async_compat::Compat<quinn::SendStream>,
}

impl Transport for QuinnTransport {}

/// A QUIC connection backed by quinn, implementing [`QuicConnection`].
#[derive(Clone, Debug)]
pub struct QuinnConnection(quinn::Connection);

impl QuinnConnection {
    pub(crate) fn new(connection: quinn::Connection) -> Self {
        Self(connection)
    }
}

impl QuicConnection for QuinnConnection {
    type BidiStream = QuinnTransport;
    type RecvStream = async_compat::Compat<quinn::RecvStream>;
    type SendStream = async_compat::Compat<quinn::SendStream>;

    async fn accept_bi(&self) -> io::Result<(u64, Self::BidiStream)> {
        let (send, recv) = self.0.accept_bi().await.map_err(conn_err)?;
        let stream_id = VarInt::from(recv.id()).into_inner();
        Ok((
            stream_id,
            QuinnTransport {
                recv: async_compat::Compat::new(recv),
                send: async_compat::Compat::new(send),
            },
        ))
    }

    async fn accept_uni(&self) -> io::Result<Self::RecvStream> {
        self.0
            .accept_uni()
            .await
            .map(async_compat::Compat::new)
            .map_err(conn_err)
    }

    async fn open_uni(&self) -> io::Result<Self::SendStream> {
        self.0
            .open_uni()
            .await
            .map(async_compat::Compat::new)
            .map_err(conn_err)
    }

    fn remote_address(&self) -> SocketAddr {
        self.0.remote_address()
    }

    fn close(&self, error_code: u64, reason: &[u8]) {
        self.0
            .close(VarInt::from_u64(error_code).unwrap_or(VarInt::MAX), reason);
    }

    fn stop_stream(&self, stream: Self::RecvStream, error_code: u64) {
        let _ = stream
            .into_inner()
            .stop(VarInt::from_u64(error_code).unwrap_or(VarInt::MAX));
    }

    fn send_datagram(&self, data: &[u8]) -> io::Result<()> {
        self.0
            .send_datagram(data.to_vec().into())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    async fn recv_datagram(&self, buf: &mut (impl Extend<u8> + Send)) -> io::Result<usize> {
        let datagram = self.0.read_datagram().await.map_err(conn_err)?;
        let len = datagram.len();
        buf.extend(datagram.iter().copied());
        Ok(len)
    }

    fn max_datagram_size(&self) -> Option<usize> {
        self.0.max_datagram_size()
    }
}

fn conn_err(e: quinn::ConnectionError) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionReset, e)
}
