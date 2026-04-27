use async_openssl::SslStream;
use openssl::{
    pkcs12::Pkcs12,
    pkey::{PKey, Private},
    ssl::{AlpnError, Ssl, SslAcceptor, SslMethod},
    x509::X509,
};
use std::{
    borrow::Cow,
    fmt::{Debug, Formatter},
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_server_common::{Acceptor, AsyncRead, AsyncWrite, Transport};

/// trillium [`Acceptor`] for openssl
#[derive(Clone)]
pub struct OpenSslAcceptor(Inner);

#[derive(Clone)]
enum Inner {
    /// Built from cert + key inside this crate. We retain the parsed openssl handles
    /// (the same ones the `SslAcceptor` already holds internally) so chain methods
    /// like `without_http2` can rebuild with a different ALPN list without keeping
    /// a copy of the raw PEM bytes around.
    Rebuildable {
        acceptor: Arc<SslAcceptor>,
        source: Source,
    },
    /// Constructed from a pre-built `SslAcceptor`. Chain methods are no-ops.
    Custom(Arc<SslAcceptor>),
}

#[derive(Clone)]
struct Source {
    cert: X509,
    chain: Vec<X509>,
    key: PKey<Private>,
    alpn: Vec<Vec<u8>>,
}

impl Debug for OpenSslAcceptor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OpenSslAcceptor")
            .field(&"<<SslAcceptor>>")
            .finish()
    }
}

impl OpenSslAcceptor {
    /// build a new `OpenSslAcceptor` from a [`SslAcceptor`]
    pub fn new(acceptor: SslAcceptor) -> Self {
        Self(Inner::Custom(Arc::new(acceptor)))
    }

    /// build a new `OpenSslAcceptor` from a PEM-encoded cert chain and PEM-encoded private key.
    ///
    /// Defaults to advertising `[h2, http/1.1]` via ALPN. Use [`Self::without_http2`] to
    /// drop HTTP/2.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use trillium_openssl::OpenSslAcceptor;
    /// const KEY: &[u8] = include_bytes!("../examples/key.pem");
    /// const CERT: &[u8] = include_bytes!("../examples/cert.pem");
    /// let acceptor = OpenSslAcceptor::from_single_cert(CERT, KEY);
    /// ```
    pub fn from_single_cert(cert: &[u8], key: &[u8]) -> Self {
        let mut chain = X509::stack_from_pem(cert)
            .expect("could not parse certificate chain")
            .into_iter();
        let leaf = chain.next().expect("certificate chain was empty");
        let chain = chain.collect();
        let key = PKey::private_key_from_pem(key).expect("could not parse private key");
        Self::from_parts(leaf, chain, key, default_alpn())
    }

    /// build a new `OpenSslAcceptor` from a pkcs12 archive and password.
    pub fn from_pkcs12(der: &[u8], password: &str) -> Self {
        let parsed = Pkcs12::from_der(der)
            .expect("could not read pkcs12 archive")
            .parse2(password)
            .expect("could not parse pkcs12 archive");
        let cert = parsed
            .cert
            .expect("pkcs12 archive contained no certificate");
        let key = parsed
            .pkey
            .expect("pkcs12 archive contained no private key");
        let chain = parsed
            .ca
            .map(|stack| stack.into_iter().collect())
            .unwrap_or_default();
        Self::from_parts(cert, chain, key, default_alpn())
    }

    fn from_parts(cert: X509, chain: Vec<X509>, key: PKey<Private>, alpn: Vec<Vec<u8>>) -> Self {
        let source = Source {
            cert,
            chain,
            key,
            alpn,
        };
        let acceptor = build_acceptor(&source);
        Self(Inner::Rebuildable {
            acceptor: Arc::new(acceptor),
            source,
        })
    }

    /// Drop `h2` from the ALPN protocol list, forcing HTTP/1.1 over TLS.
    ///
    /// Has no effect on acceptors constructed from a pre-built [`SslAcceptor`] via
    /// [`Self::new`] — those manage their own ALPN configuration.
    #[must_use]
    pub fn without_http2(self) -> Self {
        match self.0 {
            Inner::Rebuildable { mut source, .. } => {
                source.alpn.retain(|p| p != b"h2");
                let acceptor = build_acceptor(&source);
                Self(Inner::Rebuildable {
                    acceptor: Arc::new(acceptor),
                    source,
                })
            }
            other @ Inner::Custom(_) => Self(other),
        }
    }

    fn acceptor(&self) -> &SslAcceptor {
        match &self.0 {
            Inner::Rebuildable { acceptor, .. } | Inner::Custom(acceptor) => acceptor,
        }
    }
}

fn default_alpn() -> Vec<Vec<u8>> {
    vec![b"h2".to_vec(), b"http/1.1".to_vec()]
}

fn build_acceptor(source: &Source) -> SslAcceptor {
    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())
        .expect("could not build SslAcceptor");
    builder
        .set_certificate(&source.cert)
        .expect("could not set certificate");
    for ca in &source.chain {
        builder
            .add_extra_chain_cert(ca.clone())
            .expect("could not add chain certificate");
    }
    builder
        .set_private_key(&source.key)
        .expect("could not set private key");
    builder
        .check_private_key()
        .expect("private key did not match certificate");
    if !source.alpn.is_empty() {
        let server_protos = source.alpn.clone();
        builder.set_alpn_select_callback(move |_ssl, client_wire| {
            select_alpn(&server_protos, client_wire).ok_or(AlpnError::NOACK)
        });
    }
    builder.build()
}

/// Walk the wire-format ALPN list from the client and return a slice of the first protocol
/// the server prefers. Returning a subslice of `client_wire` preserves its lifetime so the
/// `set_alpn_select_callback` closure type-checks.
fn select_alpn<'c>(server: &[Vec<u8>], client_wire: &'c [u8]) -> Option<&'c [u8]> {
    let mut i = 0;
    while i < client_wire.len() {
        let len = usize::from(client_wire[i]);
        let start = i + 1;
        let end = start + len;
        if end > client_wire.len() {
            return None;
        }
        let proto = &client_wire[start..end];
        if server.iter().any(|p| p.as_slice() == proto) {
            return Some(proto);
        }
        i = end;
    }
    None
}

impl From<SslAcceptor> for OpenSslAcceptor {
    fn from(acceptor: SslAcceptor) -> Self {
        Self::new(acceptor)
    }
}

impl<Input> Acceptor<Input> for OpenSslAcceptor
where
    Input: Transport,
{
    type Error = io::Error;
    type Output = OpenSslServerTransport<Input>;

    async fn accept(&self, input: Input) -> Result<Self::Output, Self::Error> {
        let ssl = Ssl::new(self.acceptor().context()).map_err(io::Error::other)?;
        let mut stream = SslStream::new(ssl, input).map_err(io::Error::other)?;
        Pin::new(&mut stream)
            .accept()
            .await
            .map_err(io::Error::other)?;
        Ok(OpenSslServerTransport(stream))
    }
}

/// Transport for the openssl server acceptor
#[derive(Debug)]
pub struct OpenSslServerTransport<T: Unpin>(SslStream<T>);

impl<T: Unpin> OpenSslServerTransport<T> {
    /// access the contained transport (eg `TcpStream`)
    pub fn inner_transport(&self) -> &T {
        self.0.get_ref()
    }

    /// mutably access the contained transport (eg `TcpStream`)
    pub fn inner_transport_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}

impl<T: Unpin> AsRef<T> for OpenSslServerTransport<T> {
    fn as_ref(&self) -> &T {
        self.0.get_ref()
    }
}

impl<T: Unpin> AsMut<T> for OpenSslServerTransport<T> {
    fn as_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}

impl<T: Unpin> AsRef<SslStream<T>> for OpenSslServerTransport<T> {
    fn as_ref(&self) -> &SslStream<T> {
        &self.0
    }
}

impl<T: Unpin> AsMut<SslStream<T>> for OpenSslServerTransport<T> {
    fn as_mut(&mut self) -> &mut SslStream<T> {
        &mut self.0
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for OpenSslServerTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for OpenSslServerTransport<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

impl<T: Transport> Transport for OpenSslServerTransport<T> {
    fn peer_addr(&self) -> io::Result<Option<std::net::SocketAddr>> {
        self.0.get_ref().peer_addr()
    }

    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        self.0.ssl().selected_alpn_protocol().map(Cow::Borrowed)
    }
}
