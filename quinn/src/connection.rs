use async_compat::Compat;
use futures_lite::{AsyncRead, AsyncWrite};
use quinn::VarInt;
use std::{
    fmt::{self, Debug, Formatter},
    io,
    net::SocketAddr,
};
use trillium_macros::{AsyncRead, AsyncWrite};
use trillium_server_common::{
    QuicConnectionTrait, QuicTransportBidi, QuicTransportReceive, QuicTransportSend, Transport,
};

/// A bidirectional QUIC stream, combining quinn's split send/recv
/// into a single [`Transport`].
#[derive(AsyncRead, AsyncWrite)]
pub struct QuinnTransport {
    #[async_read]
    recv: Compat<quinn::RecvStream>,
    #[async_write]
    send: Compat<quinn::SendStream>,
}

impl QuinnTransport {
    fn new(recv: quinn::RecvStream, send: quinn::SendStream) -> Self {
        Self {
            recv: Compat::new(recv),
            send: Compat::new(send),
        }
    }
}

impl QuicTransportReceive for QuinnTransport {
    fn stop(&mut self, code: u64) {
        let error_code = VarInt::from_u64(code).unwrap_or_default();
        let _ = self.recv.get_mut().stop(error_code);
    }
}

impl QuicTransportSend for QuinnTransport {
    fn reset(&mut self, code: u64) {
        let error_code = VarInt::from_u64(code).unwrap_or_default();
        let _ = self.send.get_mut().reset(error_code);
    }
}

impl QuicTransportBidi for QuinnTransport {}

impl Transport for QuinnTransport {}

/// A QUIC connection backed by quinn, implementing [`QuicConnectionTrait`].
#[derive(Clone, Debug)]
pub struct QuinnConnection(quinn::Connection);

impl QuinnConnection {
    pub(crate) fn new(connection: quinn::Connection) -> Self {
        Self(connection)
    }

    pub(crate) fn inner(&self) -> &quinn::Connection {
        &self.0
    }
}

#[derive(AsyncRead)]
pub struct QuinnRecv(Compat<quinn::RecvStream>);
impl Debug for QuinnRecv {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("QuinnRecv").finish_non_exhaustive()
    }
}
impl From<quinn::RecvStream> for QuinnRecv {
    fn from(value: quinn::RecvStream) -> Self {
        Self(Compat::new(value))
    }
}
impl QuicTransportReceive for QuinnRecv {
    fn stop(&mut self, code: u64) {
        let error_code = VarInt::from_u64(code).unwrap_or_default();
        let _ = self.0.get_mut().stop(error_code);
    }
}

#[derive(AsyncWrite)]
pub struct QuinnSend(Compat<quinn::SendStream>);

impl Debug for QuinnSend {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("QuinnSend").finish_non_exhaustive()
    }
}
impl From<quinn::SendStream> for QuinnSend {
    fn from(value: quinn::SendStream) -> Self {
        Self(Compat::new(value))
    }
}
impl QuicTransportSend for QuinnSend {
    fn reset(&mut self, code: u64) {
        let error_code = VarInt::from_u64(code).unwrap_or_default();
        let _ = self.0.get_mut().reset(error_code);
    }
}

impl QuicConnectionTrait for QuinnConnection {
    type BidiStream = QuinnTransport;
    type RecvStream = QuinnRecv;
    type SendStream = QuinnSend;

    async fn accept_bidi(&self) -> io::Result<(u64, Self::BidiStream)> {
        let (send, recv) = self.0.accept_bi().await.map_err(conn_err)?;
        let stream_id = VarInt::from(recv.id()).into_inner();
        Ok((stream_id, QuinnTransport::new(recv, send)))
    }

    async fn accept_uni(&self) -> io::Result<(u64, Self::RecvStream)> {
        let recv = self.0.accept_uni().await.map_err(conn_err)?;
        let stream_id = VarInt::from(recv.id()).into_inner();
        Ok((stream_id, recv.into()))
    }

    async fn open_uni(&self) -> io::Result<(u64, Self::SendStream)> {
        let send = self.0.open_uni().await.map_err(conn_err)?;
        let stream_id = VarInt::from(send.id()).into_inner();
        Ok((stream_id, send.into()))
    }

    async fn open_bidi(&self) -> io::Result<(u64, Self::BidiStream)> {
        let (send, recv) = self.0.open_bi().await.map_err(conn_err)?;
        let stream_id = VarInt::from(recv.id()).into_inner();
        Ok((stream_id, QuinnTransport::new(recv, send)))
    }

    fn remote_address(&self) -> SocketAddr {
        self.0.remote_address()
    }

    fn close(&self, error_code: u64, reason: &[u8]) {
        self.0
            .close(VarInt::from_u64(error_code).unwrap_or(VarInt::MAX), reason);
    }

    fn send_datagram(&self, data: &[u8]) -> io::Result<()> {
        self.0
            .send_datagram(data.to_vec().into())
            .map_err(io::Error::other)
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
