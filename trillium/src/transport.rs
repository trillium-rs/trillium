use futures_lite::io::{AsyncRead, AsyncWrite};
use std::any::Any;
/**
Transport represents an AsyncRead + AsyncWrite that the http
protocol is communicated over.

Examples of this include:

* [async_std::net::TcpStream](https://docs.rs/async-std/1.9.0/async_std/net/struct.TcpStream.html)
* [async_net::TcpStream](https://docs.rs/async-net/1.6.0/async_net/struct.TcpStream.html)
* [tokio::net::TcpStream](https://docs.rs/tokio/1.6.0/tokio/net/struct.TcpStream.html) when used with [async-compat](https://docs.rs/async-compat/0.2.1/async_compat/)
* [async_rustls::TlsStream](https://docs.rs/async-rustls/0.2.0/async_rustls/enum.TlsStream.html)
* [async_tls::server::TlsStream](https://docs.rs/async-tls/0.11.0/async_tls/server/struct.TlsStream.html)
* [async_native_tls::TlsStream](https://docs.rs/async-native-tls/0.3.3/async_native_tls/struct.TlsStream.html)
* [async_net::unix::UnixStream](https://docs.rs/async-net/1.6.0/async_net/unix/struct.UnixStream.html)
* [async_std:os::unix::net::UnixStream](https://docs.rs/async-std/1.9.0/async_std/os/unix/net/struct.UnixStream.html)
* [tokio::net::UnixStream](https://docs.rs/tokio/1.6.0/tokio/net/struct.UnixStream.html) with [async-compat](https://docs.rs/async-compat/0.2.1/async_compat/)


**Note:** Transport is currently designed around
[AsyncWrite](https://docs.rs/futures/0.3.15/futures/io/trait.AsyncWrite.html)
and
[AsyncRead](https://docs.rs/futures/0.3.15/futures/io/trait.AsyncRead.html)
from the futures crate, which are different from the tokio
[AsyncRead](https://docs.rs/tokio/1.6.0/tokio/io/trait.AsyncRead.html) and [AsyncWrite](https://docs.rs/tokio/1.6.0/tokio/io/trait.AsyncWrite.html) traits. Hopefully this is a temporary
situation.

It is currently never necessary to manually implement this trait.
*/
pub trait Transport: Any + AsyncRead + AsyncWrite + Send + Sync + Unpin {
    /// in order to support downcasting from a `Box<dyn Transport>`,
    /// Transport requires implementing an `as_box_any` function.
    fn as_box_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T> Transport for T
where
    T: Any + AsyncRead + AsyncWrite + Send + Sync + Unpin,
{
    fn as_box_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}
