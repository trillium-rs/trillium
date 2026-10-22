use futures_lite::{AsyncRead, AsyncWrite};
use std::{any::Any, borrow::Cow, io::Result, net::SocketAddr, time::Duration};

/// # The interface that the http protocol is communicated over.
///
/// This trait supports several common interfaces supported by tcp
/// streams, but also can be implemented for other stream types. All trait
/// functions are currently optional.
///
/// **Note:** `Transport` uses the [`AsyncWrite`](futures_lite::AsyncWrite) and
/// [`AsyncRead`](futures_lite::AsyncRead) traits from [`futures-lite`](futures_lite), which differ
/// from the tokio `AsyncRead` / `AsyncWrite` traits. Runtime adapters handle bridging these at the
/// boundary.
#[allow(unused)]
pub trait Transport: Any + AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static {
    /// # Sets the linger duration of this transport by setting the `SO_LINGER` option
    ///
    /// See [`std::net::TcpStream::set_linger`]
    /// Optional to implement.
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports setting linger and
    /// attempting to do so is unsuccessful.
    fn set_linger(&mut self, linger: Option<Duration>) -> Result<()> {
        Ok(())
    }

    /// # Sets the value of the `TCP_NODELAY` option on this transport.
    ///
    /// See [`std::net::TcpStream::set_nodelay`].
    /// Optional to implement.
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports setting nodelay and
    /// attempting to do so is unsuccessful.
    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        Ok(())
    }

    /// # Sets the value for the `IP_TTL` option on this transport.
    ///
    /// See [`std::net::TcpStream::set_ttl`]
    /// Optional to implement
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports setting ttl and
    /// attempting to do so is unsuccessful.
    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        Ok(())
    }

    /// # Returns the socket address of the remote peer of this transport.
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports retrieving the
    /// remote peer but attempting to do so is unsuccessful.
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        Ok(None)
    }

    /// # The protocol negotiated via TLS ALPN with the peer, if any.
    ///
    /// Used by the runtime adapter to dispatch between HTTP/1.1 and HTTP/2 on the same TLS
    /// listener (`Some(b"h2")` → HTTP/2, anything else → HTTP/1.1) and by the client to pick
    /// between HTTP/1.1 and HTTP/2 when speaking to a server.
    ///
    /// The returned [`Cow`] borrows from `self` when the backend exposes the protocol as a
    /// borrowed slice (e.g. rustls) and is owned when the backend copies (e.g. native-tls). The
    /// default returns `None` so transports that don't speak TLS — or don't want to opt into
    /// ALPN-based dispatch — don't need to implement it.
    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        None
    }
}

impl Transport for Box<dyn Transport> {
    fn set_linger(&mut self, linger: Option<Duration>) -> Result<()> {
        (**self).set_linger(linger)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        (**self).set_nodelay(nodelay)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        (**self).set_ip_ttl(ttl)
    }

    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        (**self).peer_addr()
    }

    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        (**self).negotiated_alpn()
    }
}

/// Erase a transport's type. A transport that is already a `Box<dyn Transport>` passes through
/// as-is rather than gaining a second allocation and vtable hop.
pub fn box_transport<T: Transport>(transport: T) -> Box<dyn Transport> {
    same_type::<T, Box<dyn Transport>>(transport).unwrap_or_else(|transport| Box::new(transport))
}

/// The inverse of [`box_transport`].
///
/// # Panics
///
/// Panics if `T` is not the type that was erased.
pub fn unbox_transport<T: Transport>(transport: Box<dyn Transport>) -> T {
    same_type::<Box<dyn Transport>, T>(transport).unwrap_or_else(|transport| {
        *(transport as Box<dyn Any>)
            .downcast()
            .expect("attempted to downcast to the wrong transport type")
    })
}

/// `Ok` if `T` and `U` are the same type, without allocating. Each instantiation compares two
/// constant `TypeId`s, so this optimizes down to whichever branch applies.
fn same_type<T: Any, U: Any>(value: T) -> std::result::Result<U, T> {
    let mut slot = Some(value);
    if let Some(same) = (&mut slot as &mut dyn Any).downcast_mut::<Option<U>>() {
        return Ok(same.take().expect("slot was just filled"));
    }
    Err(slot.expect("slot was just filled"))
}

impl Transport for trillium_http::Synthetic {}

impl Transport for trillium_http::h2::H2Transport {
    fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        Some(Cow::Borrowed(b"h2"))
    }
}

#[cfg(test)]
mod tests {
    use super::{Transport, box_transport, unbox_transport};
    use trillium_http::Synthetic;

    fn address(transport: &dyn Transport) -> *const () {
        std::ptr::from_ref(transport).cast()
    }

    #[test]
    fn an_erased_transport_is_not_boxed_again() {
        let erased: Box<dyn Transport> = Box::new(Synthetic::from("hello"));
        let original = address(&*erased);

        let reboxed = box_transport(erased);
        assert_eq!(address(&*reboxed), original);

        let unboxed: Box<dyn Transport> = unbox_transport(reboxed);
        assert_eq!(address(&*unboxed), original);

        let _: Synthetic = unbox_transport(unboxed);
    }
}
