use crate::transport::Transport;
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    any::Any,
    fmt::{self, Debug},
    io::Result,
    net::SocketAddr,
    ops::Deref,
};
use trillium_macros::{AsyncRead, AsyncWrite};

#[allow(missing_docs)]
#[doc(hidden)]
pub(crate) trait AnyTransport: Transport + Any {
    fn as_box_any(self: Box<Self>) -> Box<dyn Any>;
    fn as_box_transport(self: Box<Self>) -> Box<dyn Transport>;
    fn as_any(&self) -> &dyn Any;
    fn as_mut_any(&mut self) -> &mut dyn Any;
    fn as_transport(&self) -> &dyn Transport;
}
impl<T: Transport + Any> AnyTransport for T {
    fn as_box_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
    fn as_box_transport(self: Box<Self>) -> Box<dyn Transport> {
        self
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_mut_any(&mut self) -> &mut dyn Any {
        self
    }
    fn as_transport(&self) -> &dyn Transport {
        self
    }
}

/**
# A type for dyn [`Transport`][crate::transport::Transport] trait objects

`BoxedTransport` represents a `Box<dyn Transport + Any>` that supports
downcasting to the original Transport. This is used in trillium to
erase the generic on Conn, in order to avoid writing `Conn<TcpStream>`
throughout an application.

**Stability Note:** This may go away at some point and be
replaced by a type alias exported from a
`trillium_server_common::Server` crate.
*/
#[derive(Debug, AsyncRead, AsyncWrite)]
pub struct BoxedTransport(Box<dyn AnyTransport>);

impl Debug for Box<dyn AnyTransport> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Box<dyn Any + Transport>")
    }
}

impl BoxedTransport {
    /**
    Create a new `BoxedTransport` from some Transport.

    ```
    use trillium_http::transport::BoxedTransport;
    use trillium_testing::TestTransport;
    // this examples uses trillium_testing::TestTransport as an
    // easily-constructed AsyncRead+AsyncWrite.
    let (test_transport, _) = TestTransport::new();

    let boxed = BoxedTransport::new(test_transport);

    let downcast: Option<Box<TestTransport>> = boxed.downcast();
    assert!(downcast.is_some());

    let boxed_again = BoxedTransport::new(*downcast.unwrap());
    assert!(boxed_again.downcast::<async_net::TcpStream>().is_none());
    ```
    */

    pub fn new<T: Transport + Any>(t: T) -> Self {
        Self(Box::new(t))
    }

    /**
    Attempt to convert the trait object into a specific transport
    T. This will only succeed if T is the type that was originally
    passed to [`BoxedTransport::new`], and will return None otherwise

    see [`BoxedTransport::new`] for example usage
    */
    #[must_use = "downcasting takes the inner transport, so you should use it"]
    pub fn downcast<T: 'static>(self) -> Option<Box<T>> {
        self.0.as_box_any().downcast().ok()
    }

    /**
    Attempt to get a reference to the trait object as a specific transport type T. This will only
    succeed if T is the type that was originally passed to [`BoxedTransport::new`], and will return
    None otherwise
    */
    pub fn downcast_ref<T: Transport>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref()
    }

    /**
    Attempt to get a mutable reference to the trait object as a specific transport type T. This
    will only succeed if T is the type that was originally passed to [`BoxedTransport::new`], and
    will return None otherwise
    */
    pub fn downcast_mut<T: Transport>(&mut self) -> Option<&mut T> {
        self.0.as_mut_any().downcast_mut()
    }
}

impl Deref for BoxedTransport {
    type Target = dyn Transport;

    fn deref(&self) -> &Self::Target {
        self.0.as_transport()
    }
}

impl Transport for BoxedTransport {
    fn set_linger(&mut self, linger: Option<std::time::Duration>) -> Result<()> {
        self.0.set_linger(linger)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        self.0.set_nodelay(nodelay)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        self.0.set_ip_ttl(ttl)
    }

    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.0.peer_addr()
    }
}
