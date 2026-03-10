use async_compat::Compat;
use futures_lite::{AsyncRead, AsyncWrite};
use quinn::VarInt;
use std::{io, net::SocketAddr};
use trillium_macros::{AsyncRead, AsyncWrite};
use trillium_server_common::{QuicConnectionTrait, Transport};

/// A bidirectional QUIC stream, combining quinn's split send/recv
/// into a single [`Transport`].
#[derive(AsyncRead, AsyncWrite)]
pub struct QuinnTransport {
    #[async_read]
    recv: Compat<quinn::RecvStream>,
    #[async_write]
    send: Compat<quinn::SendStream>,
}

impl Transport for QuinnTransport {}

/// A QUIC connection backed by quinn, implementing [`QuicConnectionTrait`].
#[derive(Clone, Debug)]
pub struct QuinnConnection(quinn::Connection);

impl QuinnConnection {
    pub(crate) fn new(connection: quinn::Connection) -> Self {
        Self(connection)
    }
}

impl QuicConnectionTrait for QuinnConnection {
    type BidiStream = QuinnTransport;
    type RecvStream = Compat<quinn::RecvStream>;
    type SendStream = Compat<quinn::SendStream>;

    async fn accept_bidi(&self) -> io::Result<(u64, Self::BidiStream)> {
        let (send, recv) = self.0.accept_bi().await.map_err(conn_err)?;
        let stream_id = VarInt::from(recv.id()).into_inner();
        Ok((
            stream_id,
            QuinnTransport {
                recv: Compat::new(recv),
                send: Compat::new(send),
            },
        ))
    }

    async fn accept_uni(&self) -> io::Result<(u64, Self::RecvStream)> {
        let recv = self.0.accept_uni().await.map_err(conn_err)?;
        let stream_id = VarInt::from(recv.id()).into_inner();
        Ok((stream_id, Compat::new(recv)))
    }

    async fn open_uni(&self) -> io::Result<(u64, Self::SendStream)> {
        let send = self.0.open_uni().await.map_err(conn_err)?;
        let stream_id = VarInt::from(send.id()).into_inner();
        Ok((stream_id, Compat::new(send)))
    }

    async fn open_bidi(&self) -> io::Result<(u64, Self::BidiStream)> {
        let (send, recv) = self.0.open_bi().await.map_err(conn_err)?;
        let stream_id = VarInt::from(recv.id()).into_inner();
        Ok((
            stream_id,
            QuinnTransport {
                recv: Compat::new(recv),
                send: Compat::new(send),
            },
        ))
    }

    fn remote_address(&self) -> SocketAddr {
        self.0.remote_address()
    }

    fn close(&self, error_code: u64, reason: &[u8]) {
        self.0
            .close(VarInt::from_u64(error_code).unwrap_or(VarInt::MAX), reason);
    }

    fn stop_uni(&self, stream: Self::RecvStream, error_code: u64) {
        let _ = stream
            .into_inner()
            .stop(VarInt::from_u64(error_code).unwrap_or(VarInt::MAX));
    }

    fn stop_bidi(&self, stream: Self::BidiStream, error_code: u64) {
        let code = VarInt::from_u64(error_code).unwrap_or(VarInt::MAX);
        let _ = stream.recv.into_inner().stop(code);
        stream.send.into_inner().reset(code).ok();
    }

    fn send_datagram(&self, data: &[u8]) -> io::Result<()> {
        self.0
            .send_datagram(data.to_vec().into())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    }

    async fn recv_datagram<F: FnOnce(&[u8]) + Send>(&self, callback: F) -> io::Result<()> {
        self.0
            .read_datagram()
            .await
            .map(|d| callback(&d))
            .map_err(conn_err)
    }

    fn max_datagram_size(&self) -> Option<usize> {
        self.0.max_datagram_size()
    }
}

fn conn_err(e: quinn::ConnectionError) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionReset, e)
}
