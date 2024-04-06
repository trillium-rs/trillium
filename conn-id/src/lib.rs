/*!
Trillium crate to add identifiers to conns.

This crate provides the following utilities:
* [`ConnId`] a handler which must be called for the rest of this crate to function
* [`log_formatter::conn_id`] a formatter to use with trillium_logger
  (note that this does not depend on the trillium_logger crate and is very lightweight
  if you do not use that crate)
* [`ConnIdExt`] an extension trait for retrieving the id from a conn

*/
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

use fastrand::Rng;
use std::{
    fmt::{Debug, Formatter, Result},
    iter::repeat_with,
    ops::Deref,
    sync::{Arc, Mutex},
};
use trillium::{Conn, Handler, HeaderName, KnownHeaderName, StateSet};

#[derive(Default)]
enum IdGenerator {
    #[default]
    Default,
    SeededFastrand(Arc<Mutex<Rng>>),
    Fn(Box<dyn Fn() -> String + Send + Sync + 'static>),
}

impl IdGenerator {
    fn generate(&self) -> Id {
        match self {
            IdGenerator::Default => Id::default(),
            IdGenerator::SeededFastrand(rng) => Id::with_rng(&mut rng.lock().unwrap()),
            IdGenerator::Fn(gen_fun) => Id(gen_fun()),
        }
    }
}

/**
Trillium handler to set a identifier for every Conn.

By default, it will use an inbound `x-request-id` request header or if
that is missing, populate a ten character random id. This handler will
set an outbound `x-request-id` header as well by default. All of this
behavior can be customized with [`ConnId::with_request_header`],
[`ConnId::with_response_header`] and [`ConnId::with_id_generator`]
*/
pub struct ConnId {
    request_header: Option<HeaderName<'static>>,
    response_header: Option<HeaderName<'static>>,
    id_generator: IdGenerator,
}

impl Debug for ConnId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("ConnId")
            .field("request_header", &self.request_header)
            .field("response_header", &self.response_header)
            .field("id_generator", &self.id_generator)
            .finish()
    }
}

impl Default for ConnId {
    fn default() -> Self {
        Self {
            request_header: Some(KnownHeaderName::XrequestId.into()),
            response_header: Some(KnownHeaderName::XrequestId.into()),
            id_generator: Default::default(),
        }
    }
}

impl ConnId {
    /**
    Constructs a new ConnId handler
    ```
    # use trillium_testing::prelude::*;
    # use trillium_conn_id::ConnId;
    let app = (ConnId::new().with_seed(1000), "ok"); // seeded for testing
    assert_ok!(
        get("/").on(&app),
        "ok",
        "x-request-id" => "J4lzoPXcT5"
    );

    assert_headers!(
        get("/")
            .with_request_header("x-request-id", "inbound")
            .on(&app),
        "x-request-id" => "inbound"
    );
    ```
    */
    pub fn new() -> Self {
        Self::default()
    }

    /**
    Specifies a request header to use. If this header is provided on
    the inbound request, the id will be used unmodified. To disable
    this behavior, see [`ConnId::without_request_header`]

    ```
    # use trillium_testing::prelude::*;
    # use trillium_conn_id::ConnId;

    let app = (
        ConnId::new().with_request_header("x-custom-id"),
        "ok"
    );

    assert_headers!(
        get("/")
            .with_request_header("x-custom-id", "inbound")
            .on(&app),
        "x-request-id" => "inbound"
    );
    ```

    */
    pub fn with_request_header(mut self, request_header: impl Into<HeaderName<'static>>) -> Self {
        self.request_header = Some(request_header.into());
        self
    }

    /**
    disables the default behavior of reusing an inbound header for
    the request id. If a ConnId is configured
    `without_request_header`, a new id will always be generated
    */
    pub fn without_request_header(mut self) -> Self {
        self.request_header = None;
        self
    }

    /**
    Specifies a response header to set. To disable this behavior, see
    [`ConnId::without_response_header`]

    ```
    # use trillium_testing::prelude::*;
    # use trillium_conn_id::ConnId;
    let app = (
        ConnId::new()
            .with_seed(1000) // for testing
            .with_response_header("x-custom-header"),
        "ok"
    );

    assert_headers!(
        get("/").on(&app),
        "x-custom-header" => "J4lzoPXcT5"
    );
    ```
    */
    pub fn with_response_header(mut self, response_header: impl Into<HeaderName<'static>>) -> Self {
        self.response_header = Some(response_header.into());
        self
    }

    /**
    Disables the default behavior of sending the conn id as a response
    header. A request id will be available within the application
    through use of [`ConnIdExt`] but will not be sent as part of the
    response.
    */
    pub fn without_response_header(mut self) -> Self {
        self.response_header = None;
        self
    }

    /**
    Provide an alternative generator function for ids. The default
    is a ten-character alphanumeric random sequence.

    ```
    # use trillium_testing::prelude::*;
    # use trillium_conn_id::ConnId;
    # use uuid::Uuid;
    let app = (
        ConnId::new().with_id_generator(|| Uuid::new_v4().to_string()),
        "ok"
    );

    // assert that the id is a valid uuid, even if we can't assert a specific value
    assert!(Uuid::parse_str(get("/").on(&app).response_headers().get_str("x-request-id").unwrap()).is_ok());
    ```
    */
    pub fn with_id_generator<F>(mut self, id_generator: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        self.id_generator = IdGenerator::Fn(Box::new(id_generator));
        self
    }

    /// seed a shared rng
    ///
    /// this is primarily useful for tests
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.id_generator = IdGenerator::SeededFastrand(Arc::new(Mutex::new(Rng::with_seed(seed))));
        self
    }

    fn generate_id(&self) -> Id {
        self.id_generator.generate()
    }
}

#[derive(Clone, Debug)]
struct Id(String);

impl Deref for Id {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(self)
    }
}

impl Default for Id {
    fn default() -> Self {
        Self(repeat_with(fastrand::alphanumeric).take(10).collect())
    }
}

impl Id {
    fn with_rng(rng: &mut Rng) -> Self {
        Self(repeat_with(|| rng.alphanumeric()).take(10).collect())
    }
}

impl Handler for ConnId {
    async fn run(&self, mut conn: Conn) -> Conn {
        let id = self
            .request_header
            .as_ref()
            .and_then(|request_header| conn.request_headers().get_str(request_header.clone()))
            .map(|request_header| Id(request_header.to_string()))
            .unwrap_or_else(|| self.generate_id());

        if let Some(ref response_header) = self.response_header {
            conn.response_headers_mut()
                .insert(response_header.clone(), id.to_string());
        }

        conn.with_state(id)
    }
}

/// Extension trait to retrieve an id generated by the [`ConnId`] handler
pub trait ConnIdExt {
    /// Retrieves the id for this conn. This method will panic if it
    /// is run before the [`ConnId`] handler.
    fn id(&self) -> &str;
}

impl<ConnLike> ConnIdExt for ConnLike
where
    ConnLike: AsRef<StateSet>,
{
    fn id(&self) -> &str {
        self.as_ref()
            .get::<Id>()
            .expect("ConnId handler must be run before calling IdConnExt::id")
    }
}

/// Formatter for the trillium_log crate
pub mod log_formatter {
    use std::borrow::Cow;

    use super::*;
    /// Formatter for the trillium_log crate. This will be `-` if
    /// there is no id on the conn.
    pub fn conn_id(conn: &Conn, _color: bool) -> Cow<'static, str> {
        conn.state::<Id>()
            .map(|id| Cow::Owned(id.0.clone()))
            .unwrap_or_else(|| Cow::Borrowed("-"))
    }
}

/// Alias for ConnId::new()
pub fn conn_id() -> ConnId {
    ConnId::new()
}

impl Debug for IdGenerator {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(match self {
            IdGenerator::Default => "IdGenerator::Default",
            IdGenerator::SeededFastrand(_) => "IdGenerator::SeededFastrand",
            IdGenerator::Fn(_) => "IdGenerator::Fn",
        })
    }
}
