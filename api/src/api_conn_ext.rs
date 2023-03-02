use mime::Mime;
use serde::{de::DeserializeOwned, Serialize};
use trillium::{Conn, KnownHeaderName::ContentType, Status};

use crate::Error;

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
            .with_status(201)
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

    This sets a status code of Status::Ok if and only if no status
    code has been explicitly set.

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
    async fn deserialize<T>(&mut self) -> Result<T, Error>
    where
        T: DeserializeOwned;
}

#[trillium::async_trait]
impl ApiConnExt for Conn {
    fn with_json(mut self, response: &impl Serialize) -> Self {
        match serde_json::to_string(&response) {
            Ok(body) => {
                if self.status().is_none() {
                    self.set_status(Status::Ok)
                }

                self.with_body(body)
                    .with_header(ContentType, "application/json")
            }

            Err(error) => self.with_state(Error::from(error)),
        }
    }

    async fn deserialize<T>(&mut self) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let body = self.request_body_string().await?;

        let content_type = self
            .headers()
            .get_str(ContentType)
            .ok_or_else(|| Error::UnsupportedMimeType {
                mime_type: "MISSING".into(),
            })
            .and_then(|mime_type| {
                mime_type
                    .parse::<Mime>()
                    .map_err(|_| Error::UnsupportedMimeType {
                        mime_type: mime_type.into(),
                    })
            })?;

        match content_type.subtype().as_str() {
            "json" => {
                let json_deserializer = &mut serde_json::Deserializer::from_str(&body);
                Ok(serde_path_to_error::deserialize::<_, T>(json_deserializer)?)
            }

            #[cfg(feature = "forms")]
            "x-www-form-urlencoded" => {
                let body = form_urlencoded::parse(body.as_bytes());
                let deserializer = serde_urlencoded::Deserializer::new(body);
                Ok(serde_path_to_error::deserialize::<_, T>(deserializer)?)
            }

            _ => Err(Error::UnsupportedMimeType {
                mime_type: content_type.to_string(),
            }),
        }
    }
}
