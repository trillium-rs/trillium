/*!
This crate represents a first pass at a utility crate for creating APIs with trillium.rs.

## Formats supported:

Currently, this crate supports *receiving* `application/json` and
`application/x-form-www-urlencoded` by default. To disable
`application/x-form-www-urlencoded` support, use `default-features =
false`.

This crate currently only supports sending json responses, but may
eventually add `Accepts` negotiation and further outbound response
content types.

The [`ApiConnExt`] extension trait and [`ApiHandler`] can be used
independently or in combination.

[`ApiHandler`] provides an easy way to deserialize a single type from
the request body, with a default approach to handling invalid
serialization. ApiHandler does not handle serializing responses, so is
best used in conjunction with [`ApiConnExt::with_json`]. If you need
custom handling for deserialization errors, use
[`ApiConnExt::deserialize`] instead of [`ApiHandler`].
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

use serde::{de::DeserializeOwned, Serialize};
pub use serde_json::{json, Value};
use std::{fmt::Debug, future::Future, marker::PhantomData};
use trillium::{async_trait, conn_try, Conn, Handler, KnownHeaderName::ContentType};

/**
Trillium API handler

Construct with [`api`] or [`ApiHandler::new`] and an async
function that takes a [`Conn`] and any type that you've defined
which implements [`DeserializeOwned`] and returns the [`Conn`].

## Examples

```
use trillium_api::{ApiHandler, ApiConnExt};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct BlogPost {
    title: String,
    body: String,
}

# async fn persist(blog_post: &mut BlogPost) -> Result<(), ()> { Ok(()) }
async fn blog_post_handler(conn: trillium::Conn, mut blog_post: BlogPost) -> trillium::Conn {
    match persist(&mut blog_post).await {
        Ok(_) => conn.with_json(&blog_post),
        Err(_) => conn.with_json(&blog_post).with_status(406),
    }
}

let handler = ApiHandler::new(blog_post_handler); // equivalently, api(blog_post_handler)
# use trillium_testing::prelude::*;

/// accepts json
assert_ok!(
    post("/")
        .with_request_body(r#"{ "title": "introducing trillium.rs", "body": "it's like plug, for async rust" }"#)
        .with_request_header("content-type", "application/json")
        .on(&handler),
    "{\"title\":\"introducing trillium.rs\",\"body\":\"it's like plug, for async rust\"}",
    "content-type" => "application/json"
);


/// accepts x-www-form-urlencoded
assert_ok!(
    post("/")
        .with_request_body(r#"title=introducing+trillium.rs&body=it%27s+like+plug%2C+for+async+rust"#)
        .with_request_header("content-type", "application/x-www-form-urlencoded")
        .on(&handler),
    "{\"title\":\"introducing trillium.rs\",\"body\":\"it's like plug, for async rust\"}",
    "content-type" => "application/json"
);
```

*/

#[derive(Default, Debug)]
pub struct ApiHandler<F, BodyType> {
    handler_fn: F,
    body_type: PhantomData<BodyType>,
}

/// Convenience function to build a trillium api handler. This is an
/// alias for [`ApiHandler::new`].
pub fn api<F, Fut, BodyType>(handler_fn: F) -> ApiHandler<F, BodyType>
where
    BodyType: DeserializeOwned + Send + Sync + 'static,
    F: Fn(Conn, BodyType) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn> + Send + 'static,
{
    ApiHandler::new(handler_fn)
}

impl<F, Fut, BodyType> ApiHandler<F, BodyType>
where
    BodyType: DeserializeOwned + Send + Sync + 'static,
    F: Fn(Conn, BodyType) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn> + Send + 'static,
{
    /// Build a new API handler for the given async function. This is
    /// aliased as [`api`].
    pub fn new(handler_fn: F) -> Self {
        Self {
            handler_fn,
            body_type: PhantomData::default(),
        }
    }
}

#[async_trait]
impl<F, Fut, BodyType> Handler for ApiHandler<F, BodyType>
where
    BodyType: DeserializeOwned + Send + Sync + 'static,
    F: Fn(Conn, BodyType) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn> + Send + 'static,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        match conn.deserialize::<BodyType>().await {
            Ok(b) => (self.handler_fn)(conn, b).await,
            Err(e) => conn.with_json(&e).with_status(422).halt(),
        }
    }
}

/// Extension trait that adds api methods to [`trillium::Conn`]
#[trillium::async_trait]
pub trait ApiConnExt {
    /**
    Sends a json response body. This sets a status code of 200,
    serializes the body with serde_json, sets the content-type to
    application/json, and [halts](trillium::Conn::halt) the
    conn. If serialization fails, a 500 status code is sent as per
    [`trillium::conn_try`]


    ## Examples

    ```
    use trillium_api::{json, ApiConnExt};
    async fn handler(conn: trillium::Conn) -> trillium::Conn {
        conn.with_json(&json!({ "json macro": "is reexported" }))
    }

    # use trillium_testing::prelude::*;
    assert_ok!(
        get("/").on(&handler),
        r#"{"json macro":"is reexported"}"#,
        "content-type" => "application/json"
    );
    ```

    ### overriding status code
    ```
    use trillium_api::ApiConnExt;
    use serde::Serialize;

    #[derive(Serialize)]
    struct ApiResponse {
       string: &'static str,
       number: usize
    }

    async fn handler(conn: trillium::Conn) -> trillium::Conn {
        conn.with_json(&ApiResponse { string: "not the most creative example", number: 100 })
            .with_status(201) // note that this has to be chained _after_ the with_json call
    }

    # use trillium_testing::prelude::*;
    assert_response!(
        get("/").on(&handler),
        Status::Created,
        r#"{"string":"not the most creative example","number":100}"#,
        "content-type" => "application/json"
    );
    ```
    */
    fn with_json(self, response: &impl Serialize) -> Self;

    /**
    Attempts to deserialize a type from the request body, based on the
    request content type.

    By default, both application/json and
    application/x-www-form-urlencoded are supported, and future
    versions may add accepted request content types. Please open an
    issue if you need to accept another content type.


    To exclusively accept application/json, disable default features
    on this crate.


    ## Examples

    ### Deserializing to [`Value`]

    ```
    use trillium_api::{ApiConnExt, Value};

    async fn handler(mut conn: trillium::Conn) -> trillium::Conn {
        let value: Value = trillium::conn_try!(conn.deserialize().await, conn);
        conn.with_json(&value)
    }

    # use trillium_testing::prelude::*;
    assert_ok!(
        post("/")
            .with_request_body(r#"key=value"#)
            .with_request_header("content-type", "application/x-www-form-urlencoded")
            .on(&handler),
        r#"{"key":"value"}"#,
        "content-type" => "application/json"
    );

    ```

    ### Deserializing a concrete type

    ```
    use trillium_api::ApiConnExt;

    #[derive(serde::Deserialize)]
    struct KvPair { key: String, value: String }

    async fn handler(mut conn: trillium::Conn) -> trillium::Conn {
        match conn.deserialize().await {
            Ok(KvPair { key, value }) => {
                conn.with_status(201)
                    .with_body(format!("{} is {}", key, value))
                    .halt()
            }

            Err(_) => conn.with_status(422).with_body("nope").halt()
        }
    }

    # use trillium_testing::prelude::*;
    assert_response!(
        post("/")
            .with_request_body(r#"key=name&value=trillium"#)
            .with_request_header("content-type", "application/x-www-form-urlencoded")
            .on(&handler),
        Status::Created,
        r#"name is trillium"#,
    );

    assert_response!(
        post("/")
            .with_request_body(r#"name=trillium"#)
            .with_request_header("content-type", "application/x-www-form-urlencoded")
            .on(&handler),
        Status::UnprocessableEntity,
        r#"nope"#,
    );


    ```

    */
    async fn deserialize<T>(&mut self) -> Result<T, Value>
    where
        T: DeserializeOwned;
}

#[trillium::async_trait]
impl ApiConnExt for Conn {
    fn with_json(self, response: &impl Serialize) -> Self {
        let body = conn_try!(serde_json::to_string(&response), self);
        self.ok(body).with_header(ContentType, "application/json")
    }

    async fn deserialize<T>(&mut self) -> Result<T, Value>
    where
        T: DeserializeOwned,
    {
        let body = self
            .request_body_string()
            .await
            .map_err(|e| json!({ "errorType": "io error", "message": e.to_string() }))?;

        let content_type = self
            .headers()
            .get_str(ContentType)
            .and_then(|c| c.parse().ok())
            .unwrap_or(mime::APPLICATION_JSON);

        match content_type.subtype().as_str() {
            "json" => serde_json::from_str::<T>(&body).map_err(|e| {
                json!({
                    "input": body,
                    "line": e.line(),
                    "column": e.column(),
                    "message": e.to_string()
                })
            }),

            #[cfg(feature = "forms")]
            "x-www-form-urlencoded" => serde_urlencoded::from_str::<T>(&body)
                .map_err(|e| json!({ "input": body, "message": e.to_string() })),

            _ => Err(json!({
                "errorType": format!("unknown content type {content_type}")
            })),
        }
    }
}
