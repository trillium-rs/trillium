use crate::{Error, Transport};
use futures_lite::AsyncRead;
use trillium_http::ReceivedBody;
use trillium_macros::AsyncRead;

/// A received request body
///
/// This type represents a body that will be read from the underlying transport
///
/// ```rust
/// # use trillium_testing::TestServer;
/// # trillium_testing::block_on(async move {
/// let app = TestServer::new(|mut conn: trillium::Conn| async move {
///     let body = conn.request_body(); // 100-continue sent lazily on first read if needed
///     let Ok(body_string) = body.with_max_len(1024).read_string().await else {
///         return conn.with_status(500);
///     };
///
///     conn.with_body(format!("received: {body_string}"))
/// })
/// .await;
///
/// app.get("/").await.assert_body("received: ");
/// app.post("/")
///     .with_body("hello")
///     .await
///     .assert_body("received: hello");
/// # });
/// ```
///
/// ## Bounds checking
///
/// Every `RequestBody` has a maximum length beyond which it will return an error, expressed as a
/// u64. To override this on the specific `RequestBody`, use [`RequestBody::with_max_len`] or
/// [`RequestBody::set_max_len`]
#[derive(AsyncRead, Debug)]
pub struct RequestBody<'a>(ReceivedBody<'a, Box<dyn Transport>>);

impl RequestBody<'_> {
    /// Similar to [`RequestBody::read_string`], but returns the raw bytes. This is useful for
    /// bodies that are not text.
    ///
    /// You can use this in conjunction with `encoding` if you need different handling of malformed
    /// character encoding than the lossy conversion provided by [`RequestBody::read_string`].
    ///
    /// An empty or nonexistent body will yield an empty Vec, not an error.
    ///
    /// # Errors
    ///
    /// This will return an error if there is an IO error on the underlying transport such as a
    /// disconnect
    ///
    /// This will also return an error if the length exceeds the maximum length. To configure the
    /// value on this specific request body, use [`RequestBody::with_max_len`] or
    /// [`RequestBody::set_max_len`]
    pub async fn read_bytes(self) -> Result<Vec<u8>, Error> {
        self.0.read_bytes().await
    }

    /// # Reads the entire body to `String`.
    ///
    /// This uses the encoding determined by the content-type (mime) charset. If an encoding problem
    /// is encountered, the String returned by [`RequestBody::read_string`] will contain utf8
    /// replacement characters.
    ///
    /// Note that this can only be performed once per Conn, as the underlying data is not cached
    /// anywhere. This is the only copy of the body contents.
    ///
    /// An empty or nonexistent body will yield an empty String, not an error
    ///
    /// # Errors
    ///
    /// This will return an error if there is an IO error on the
    /// underlying transport such as a disconnect
    ///
    ///
    /// This will also return an error if the length exceeds the maximum length. To configure the
    /// value on this specific request body, use [`RequestBody::with_max_len`] or
    /// [`RequestBody::set_max_len`].
    pub async fn read_string(self) -> Result<String, Error> {
        self.0.read_string().await
    }

    /// Set the maximum content length to read, returning self
    ///
    /// This protects against an memory-use denial-of-service attack wherein an untrusted peer sends
    /// an unbounded request body. This is especially important when using
    /// [`RequestBody::read_string`] and [`RequestBody::read_bytes`] instead of streaming with
    /// `AsyncRead`.
    ///
    /// The default value can be found documented [in the trillium-http
    /// crate](https://docs.trillium.rs/trillium_http/struct.httpconfig#received_body_max_len)
    #[must_use]
    pub fn with_max_len(mut self, max_len: u64) -> Self {
        self.0.set_max_len(max_len);
        self
    }

    /// Set the maximum content length to read
    ///
    /// This protects against an memory-use denial-of-service attack wherein an untrusted peer sends
    /// an unbounded request body. This is especially important when using
    /// [`RequestBody::read_string`] and [`RequestBody::read_bytes`] instead of streaming with
    /// `AsyncRead`.
    ///
    /// The default value can be found documented [in the trillium-http
    /// crate](https://docs.trillium.rs/trillium_http/struct.httpconfig#received_body_max_len)
    pub fn set_max_len(&mut self, max_len: u64) -> &mut Self {
        self.0.set_max_len(max_len);
        self
    }

    /// The content-length of this body, if available.
    ///
    /// This value usually is derived from the content-length header. If the request that this body
    /// is attached to uses transfer-encoding chunked, this will be None.
    pub fn content_length(&self) -> Option<u64> {
        self.0.content_length()
    }
}

impl<'a> From<RequestBody<'a>> for ReceivedBody<'a, Box<dyn Transport>> {
    fn from(value: RequestBody<'a>) -> Self {
        value.0
    }
}

impl<'a> From<ReceivedBody<'a, Box<dyn Transport>>> for RequestBody<'a> {
    fn from(received_body: ReceivedBody<'a, Box<dyn Transport>>) -> Self {
        Self(received_body)
    }
}
