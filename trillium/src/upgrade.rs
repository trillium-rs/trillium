use crate::{Headers, HttpContext, Method, Transport, TypeSet, Version};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    mem,
    net::IpAddr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
};
use trillium_http::Swansong;
use trillium_macros::{AsyncRead, AsyncWrite};

/// # A HTTP protocol upgrade
#[derive(Debug, AsyncWrite, AsyncRead)]
pub struct Upgrade {
    #[async_write]
    #[async_read]
    inner: trillium_http::Upgrade<Box<dyn Transport>>,
    path_frames: PathFrames,
}

/// A path-frame stack mutable through `&self`, because [`Handler::has_upgrade`] takes
/// `&Upgrade`.
///
/// [`Upgrade::path`] lends `&str`s out of this structure for the lifetime of `&self`, so a
/// frame may never be freed or moved while the `Upgrade` is alive — even after `pop`. Storage
/// is therefore an append-only chain of `OnceLock` links (`OnceLock::set` takes `&self`), and
/// each node records its parent's position, making the chain a persistent stack: `top` is the
/// 1-based chain position of the current top frame (0 = empty, full path), push appends a node
/// whose parent is the current top, and pop moves `top` to the parent — popped frames stay
/// allocated until the `Upgrade` drops.
///
/// [`Handler::has_upgrade`]: crate::Handler::has_upgrade
#[derive(Debug, Default)]
struct PathFrames {
    head: OnceLock<Box<FrameNode>>,
    top: AtomicUsize,
}

#[derive(Debug)]
struct FrameNode {
    frame: String,
    /// 1-based chain position of the frame below this one on the stack; 0 = stack bottom
    parent: usize,
    next: OnceLock<Box<Self>>,
}

impl PathFrames {
    fn get(&self, position: usize) -> Option<&FrameNode> {
        let steps = position.checked_sub(1)?;
        let mut node = self.head.get()?;
        for _ in 0..steps {
            node = node.next.get()?;
        }
        Some(node)
    }

    fn top_frame(&self) -> Option<&str> {
        match self.top.load(Relaxed) {
            0 => None,
            top => self.get(top).map(|node| &*node.frame),
        }
    }

    fn push(&self, frame: String) {
        let parent = self.top.load(Relaxed);
        let mut node = Box::new(FrameNode {
            frame,
            parent,
            next: OnceLock::new(),
        });
        let mut position = 1;
        let mut lock = &self.head;
        loop {
            while let Some(occupied) = lock.get() {
                lock = &occupied.next;
                position += 1;
            }
            match lock.set(node) {
                Ok(()) => break,
                // a concurrent push claimed this link between the get and the set; the cell is
                // now (or is about to finish being) initialized, so resume walking from it
                Err(rejected) => node = rejected,
            }
        }
        self.top.store(position, Relaxed);
    }

    fn pop(&self) {
        let top = self.top.load(Relaxed);
        if let Some(node) = self.get(top) {
            self.top.store(node.parent, Relaxed);
        }
    }
}

impl<T: Transport + 'static> From<trillium_http::Upgrade<T>> for Upgrade {
    fn from(value: trillium_http::Upgrade<T>) -> Self {
        Self {
            inner: value.map_transport(|t| Box::new(t) as Box<dyn Transport>),
            path_frames: PathFrames::default(),
        }
    }
}

impl<T: Transport + 'static> From<trillium_http::Conn<T>> for Upgrade {
    fn from(value: trillium_http::Conn<T>) -> Self {
        trillium_http::Upgrade::from(value).into()
    }
}

impl From<crate::Conn> for Upgrade {
    fn from(value: crate::Conn) -> Self {
        Self {
            inner: value.inner.into(),
            path_frames: PathFrames::default(),
        }
    }
}

impl AsRef<trillium_http::Upgrade<Box<dyn Transport>>> for Upgrade {
    fn as_ref(&self) -> &trillium_http::Upgrade<Box<dyn Transport>> {
        &self.inner
    }
}

impl AsMut<trillium_http::Upgrade<Box<dyn Transport>>> for Upgrade {
    fn as_mut(&mut self) -> &mut trillium_http::Upgrade<Box<dyn Transport>> {
        &mut self.inner
    }
}

impl Upgrade {
    /// Borrows the HTTP request headers
    pub fn request_headers(&self) -> &Headers {
        self.inner.received_headers()
    }

    /// Take the HTTP request headers
    pub fn take_request_headers(&mut self) -> Headers {
        mem::take(self.inner.received_headers_mut())
    }

    /// Returns a copy of the HTTP request method
    pub fn method(&self) -> Method {
        self.inner.method()
    }

    /// Borrows the state accumulated on the Conn before negotiating the upgrade
    pub fn state(&self) -> &TypeSet {
        self.inner.state()
    }

    /// Takes the [`TypeSet`] accumulated on the Conn before negotiating the upgrade
    pub fn take_state(&mut self) -> TypeSet {
        mem::take(self.inner.state_mut())
    }

    /// Mutably borrow the [`TypeSet`] accumulated on the Conn before negotiating the upgrade
    pub fn state_mut(&mut self) -> &mut TypeSet {
        self.inner.state_mut()
    }

    /// Borrows the underlying transport
    pub fn transport(&self) -> &dyn Transport {
        self.inner.transport().as_ref()
    }

    /// Mutably borrow the underlying transport
    ///
    /// This returns a tuple of (buffered bytes, transport) in order to make salient the requirement
    /// to handle any buffered bytes before using the transport directly.
    pub fn transport_mut(&mut self) -> (&[u8], &mut dyn Transport) {
        let (buffer, transport) = self.inner.buffer_and_transport_mut();
        (&*buffer, &mut **transport)
    }

    /// Consumes self, returning the underlying transport
    ///
    /// This returns a tuple of (buffered bytes, transport) in order to make salient the requirement
    /// to handle any buffered bytes before using the transport directly.
    pub fn into_transport(mut self) -> (Vec<u8>, Box<dyn Transport>) {
        let buffer = self.inner.take_buffer();
        (buffer, self.inner.into_transport())
    }

    /// Returns a copy of the peer IP address of the connection, if available
    pub fn peer_ip(&self) -> Option<IpAddr> {
        self.inner.peer_ip()
    }

    /// Borrows the :authority HTTP/3 pseudo-header
    pub fn authority(&self) -> Option<&str> {
        self.inner.authority()
    }

    /// Borrows the :scheme HTTP/3 pseudo-header
    pub fn scheme(&self) -> Option<&str> {
        self.inner.scheme()
    }

    /// Borrows the :protocol HTTP/3 pseudo-header
    pub fn protocol(&self) -> Option<&str> {
        self.inner.protocol()
    }

    /// Borrows the HTTP version
    pub fn http_version(&self) -> &Version {
        self.inner.http_version()
    }

    /// Returns a copy of whether this connection was deemed secure by the handler stack
    pub fn is_secure(&self) -> bool {
        self.inner.is_secure()
    }

    /// Borrows the shared state [`TypeSet`] for this application
    pub fn shared_state(&self) -> &TypeSet {
        self.inner.shared_state()
    }

    /// Returns the HTTP request path up to but excluding any query component
    ///
    /// As with [`Conn::path`][crate::Conn::path], this may not represent the entire http request
    /// path if this upgrade is being dispatched through nested routers: after
    /// [`push_path`][Upgrade::push_path], it returns the pushed path remainder relative to the
    /// enclosing router mount.
    pub fn path(&self) -> &str {
        self.path_frames
            .top_frame()
            .unwrap_or_else(|| self.inner.path())
    }

    /// for router implementations. pushes a route segment onto the path, the upgrade-dispatch
    /// analog of [`Conn::push_path`][crate::Conn::push_path] — see its documentation for the
    /// contract shared by all of a handler's hooks.
    ///
    /// Takes a shared reference because [`Handler::has_upgrade`][crate::Handler::has_upgrade]
    /// does. To make that possible while [`path`][Upgrade::path] lends out plain `&str`s, frames
    /// removed by [`pop_path`][Upgrade::pop_path] remain allocated until the `Upgrade` drops.
    pub fn push_path(&self, path: String) {
        self.path_frames.push(path);
    }

    /// for router implementations. removes a route segment pushed by
    /// [`push_path`][Upgrade::push_path], the upgrade-dispatch analog of
    /// [`Conn::pop_path`][crate::Conn::pop_path]
    pub fn pop_path(&self) {
        self.path_frames.pop();
    }

    /// Retrieves the query component of the path
    pub fn querystring(&self) -> &str {
        self.inner.querystring()
    }

    /// Retrieves a cloned [`Swansong`] graceful shutdown controller
    pub fn swansong(&self) -> Swansong {
        self.inner.context().swansong().clone()
    }

    /// Retrieves a clone of the [`HttpContext`] for this upgrade
    pub fn context(&self) -> Arc<HttpContext> {
        self.inner.context().clone()
    }

    /// Returns a clone of the H3 connection, if any
    pub fn h3_connection(&self) -> Option<Arc<trillium_http::h3::H3Connection>> {
        self.inner.h3_connection().cloned()
    }

    /// Inbound trailers, populated conditionally when we have read this upgrade to completion
    pub fn request_trailers(&self) -> Option<&Headers> {
        self.inner.received_trailers()
    }

    /// Emit trailing headers and finish the outbound stream. Consumes `self`; further
    /// writes are statically prevented.
    ///
    /// Per-protocol behavior:
    /// - HTTP/1.1 with `Transfer-Encoding: chunked`: writes the last-chunk marker (`0\r\n`), the
    ///   trailer section, and a final CRLF, then closes the transport.
    /// - HTTP/2: enqueues a trailing `HEADERS` frame with `END_STREAM` via the connection driver
    ///   and returns. The driver finishes the stream after draining any pending DATA frames.
    /// - HTTP/3: encodes a trailing `HEADERS` frame via QPACK, writes it to the stream, then closes
    ///   the stream (QUIC `FIN`).
    /// - HTTP/1.1 without chunked encoding (raw upgrade, CONNECT tunnel, websocket-over-h1):
    ///   trailers can't be expressed on the wire; dropped with a `log::warn!` and `Ok(())`
    ///   returned.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`std::io::Error`] when the wire write fails, `BrokenPipe` if
    /// the stream has already been closed, and `NotConnected` if the carried
    /// `ProtocolSession` is missing the expected driver for h2/h3.
    pub async fn send_trailers(self, trailers: Headers) -> std::io::Result<()> {
        self.inner.send_trailers(trailers).await
    }
}

#[cfg(test)]
mod tests {
    use super::PathFrames;

    #[test]
    fn path_frames_push_pop_and_divergence() {
        let frames = PathFrames::default();
        assert_eq!(frames.top_frame(), None);

        frames.pop(); // empty pop is a no-op
        assert_eq!(frames.top_frame(), None);

        frames.push("a".into());
        frames.push("b".into());
        assert_eq!(frames.top_frame(), Some("b"));

        let borrowed_before_pop = frames.top_frame().unwrap();
        frames.pop();
        assert_eq!(frames.top_frame(), Some("a"));
        assert_eq!(borrowed_before_pop, "b"); // still valid after pop

        frames.push("c".into()); // diverge from the popped "b"
        assert_eq!(frames.top_frame(), Some("c"));

        frames.pop();
        frames.pop();
        assert_eq!(frames.top_frame(), None);

        frames.push("d".into());
        assert_eq!(frames.top_frame(), Some("d"));
    }
}
