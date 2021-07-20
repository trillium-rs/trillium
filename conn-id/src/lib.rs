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

use extension_trait::extension_trait;
use std::{
    fmt::{Debug, Formatter, Result},
    iter::repeat_with,
    ops::Deref,
};
use trillium::{async_trait, Conn, Handler};

/**
Trillium handler to set a identifier for every Conn.

By default, it will use an inbound `x-request-id` request header or if
that is missing, populate a ten character random id. This handler will
set an outbound `x-request-id` header as well by default. All of this
behavior can be customized with [`ConnId::with_request_header`],
[`ConnId::with_response_header`] and [`ConnId::with_id_generator`]
*/
pub struct ConnId {
    request_header: Option<&'static str>,
    response_header: Option<&'static str>,
    id_generator: Option<Box<dyn Fn() -> String + Send + Sync + 'static>>,
}

impl Debug for ConnId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("ConnId")
            .field("request_header", &self.request_header)
            .field("response_header", &self.response_header)
            .field(
                "id_generator",
                &if self.id_generator.is_some() {
                    "Some(id generator fn)"
                } else {
                    "None"
                },
            )
            .finish()
    }
}

impl Default for ConnId {
    fn default() -> Self {
        Self {
            request_header: Some("x-request-id"),
            response_header: Some("x-request-id"),
            id_generator: None,
        }
    }
}

impl ConnId {
    /**
    Constructs a new ConnId handler
    ```
    # use trillium_testing::prelude::*;
    # use trillium_conn_id::ConnId;

    let app = (ConnId::new(), "ok");

    assert_eq!(get("/").on(&app).headers_mut()["x-request-id"].as_str().len(), 10);

    assert_headers!(
        get("/")
            .with_request_header(("x-request-id", "inbound"))
            .on(&app),
        "x-request-id" => "inbound"
    );
    ```
    */
    pub fn new() -> Self {
        Self::default()
    }

    /**
    Specifies a request header to use. If this header is provided
    on the inbound request, the id will be used unmodified. If
    None is provided, a new id will always be generated

    ```
    # use trillium_testing::prelude::*;
    # use trillium_conn_id::ConnId;

    let app = (
        ConnId::new().with_request_header(Some("x-custom-id")),
        "ok"
    );

    assert_headers!(
        get("/")
            .with_request_header(("x-custom-id", "inbound"))
            .on(&app),
        "x-request-id" => "inbound"
    );
    ```

    */
    pub fn with_request_header(mut self, request_header: Option<&'static str>) -> Self {
        self.request_header = request_header;
        self
    }

    /**
    Specifies a response header to set. If None is provided, a
    request id will be available within the application through
    use of [`ConnIdExt`] but will not be sent as part of the
    response.

    ```
    # use trillium_testing::prelude::*;
    # use trillium_conn_id::{ConnId, ConnIdExt};

    let app = (
        ConnId::new().with_response_header(Some("x-custom-header")),
        "ok"
    );

    let mut conn = get("/").on(&app);
    let id = String::from(conn.id());

    assert_headers!(&mut conn, "x-custom-header" => &*id);
    ```
    */
    pub fn with_response_header(mut self, response_header: Option<&'static str>) -> Self {
        self.response_header = response_header;
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
    assert!(Uuid::parse_str(get("/").on(&app).headers_mut()["x-request-id"].as_str()).is_ok());
    ```
    */
    pub fn with_id_generator<F>(mut self, id_generator: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        self.id_generator = Some(Box::new(id_generator));
        self
    }

    fn generate_id(&self) -> Id {
        if let Some(ref id_generator) = self.id_generator {
            Id(id_generator())
        } else {
            Id::default()
        }
    }
}

#[derive(Clone, Debug)]
struct Id(String);

impl Deref for Id {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(&*self)
    }
}

impl Default for Id {
    fn default() -> Self {
        Self(repeat_with(fastrand::alphanumeric).take(10).collect())
    }
}

#[async_trait]
impl Handler for ConnId {
    async fn run(&self, mut conn: Conn) -> Conn {
        let id = self
            .request_header
            .and_then(|request_header| conn.headers().get(request_header))
            .map(|request_header| Id(request_header.as_str().to_string()))
            .unwrap_or_else(|| self.generate_id());

        if let Some(response_header) = self.response_header {
            conn.headers_mut().insert(response_header, &*id);
        }

        conn.with_state(id)
    }
}

/// Extension trait to retrieve an id generated by the [`ConnId`] handler
#[extension_trait]
pub impl ConnIdExt for Conn {
    /// Retrieves the id for this conn. This method will panic if it
    /// is run before the [`ConnId`] handler.
    fn id(&self) -> &str {
        &*self
            .state::<Id>()
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
