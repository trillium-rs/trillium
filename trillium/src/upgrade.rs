use crate::{Headers, Method, ServerConfig, Transport, TypeSet, Version};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{net::IpAddr, sync::Arc};
use trillium_http::Swansong;
use trillium_macros::{AsyncRead, AsyncWrite};

/// # A HTTP protocol upgrade
#[derive(Debug, AsyncWrite, AsyncRead)]
pub struct Upgrade(trillium_http::Upgrade<Box<dyn Transport>>);

impl<T: Transport + 'static> From<trillium_http::Upgrade<T>> for Upgrade {
    fn from(value: trillium_http::Upgrade<T>) -> Self {
        Self(value.map_transport(|t| Box::new(t) as Box<dyn Transport>))
    }
}

impl<T: Transport + 'static> From<trillium_http::Conn<T>> for Upgrade {
    fn from(value: trillium_http::Conn<T>) -> Self {
        trillium_http::Upgrade::from(value).into()
    }
}

impl From<crate::Conn> for Upgrade {
    fn from(value: crate::Conn) -> Self {
        Self(value.inner.into())
    }
}

impl Upgrade {
    /// Borrows the HTTP request headers
    pub fn request_headers(&self) -> &Headers {
        self.0.request_headers()
    }

    /// Take the HTTP request headers
    pub fn take_request_headers(&mut self) -> Headers {
        std::mem::take(self.0.request_headers_mut())
    }

    /// Returns a copy of the HTTP request method
    pub fn method(&self) -> Method {
        self.0.method()
    }

    /// Borrows the state accumulated on the Conn before negotiating the upgrade
    pub fn state(&self) -> &TypeSet {
        self.0.state()
    }

    /// Takes the [`TypeSet`] accumulated on the Conn before negotiating the upgrade
    pub fn take_state(&mut self) -> TypeSet {
        std::mem::take(self.0.state_mut())
    }

    /// Mutably borrow the [`TypeSet`] accumulated on the Conn before negotiating the upgrade
    pub fn state_mut(&mut self) -> &mut TypeSet {
        self.0.state_mut()
    }

    /// Borrows the underlying transport
    pub fn transport(&self) -> &dyn Transport {
        self.0.transport().as_ref()
    }

    /// Mutably borrow the underlying transport
    ///
    /// This returns a tuple of (buffered bytes, transport) in order to make salient the requirement
    /// to handle any buffered bytes before using the transport directly.
    pub fn transport_mut(&mut self) -> (&[u8], &mut dyn Transport) {
        let (buffer, transport) = self.0.buffer_and_transport_mut();
        (&*buffer, &mut **transport)
    }

    /// Consumes self, returning the underlying transport
    ///
    /// This returns a tuple of (buffered bytes, transport) in order to make salient the requirement
    /// to handle any buffered bytes before using the transport directly.
    pub fn into_transport(mut self) -> (Vec<u8>, Box<dyn Transport>) {
        let buffer = self.0.take_buffer();
        (buffer, self.0.into_transport())
    }

    /// Returns a copy of the peer IP address of the connection, if available
    pub fn peer_ip(&self) -> Option<IpAddr> {
        self.0.peer_ip()
    }

    /// Borrows the :authority HTTP/3 pseudo-header
    pub fn authority(&self) -> Option<&str> {
        self.0.authority()
    }

    /// Borrows the :scheme HTTP/3 pseudo-header
    pub fn scheme(&self) -> Option<&str> {
        self.0.scheme()
    }

    /// Borrows the :protocol HTTP/3 pseudo-header
    pub fn protocol(&self) -> Option<&str> {
        self.0.protocol()
    }

    /// Borrows the HTTP version
    pub fn http_version(&self) -> &Version {
        self.0.http_version()
    }

    /// Returns a copy of whether this connection was deemed secure by the handler stack
    pub fn is_secure(&self) -> bool {
        self.0.is_secure()
    }

    /// Borrows the shared state [`TypeSet`] for this application
    pub fn shared_state(&self) -> &TypeSet {
        self.0.shared_state()
    }

    /// Returns the HTTP request path up to but excluding any query component
    pub fn path(&self) -> &str {
        self.0.path()
    }

    /// Retrieves the query component of the path
    pub fn querystring(&self) -> &str {
        self.0.querystring()
    }

    /// Retrieves a cloned [`Swansong`] graceful shutdown controller
    pub fn swansong(&self) -> Swansong {
        self.0.server_config().swansong().clone()
    }

    /// Retrieves a clone of the [`ServerConfig`] for this upgrade
    pub fn server_config(&self) -> Arc<ServerConfig> {
        self.0.server_config().clone()
    }

    /// Returns a clone of the H3 connection, if any
    pub fn h3_connection(&self) -> Option<Arc<trillium_http::h3::H3Connection>> {
        self.0.h3_connection().cloned()
    }
}
