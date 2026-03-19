use crate::{Error, Result};
use mime::Mime;
use serde::{Serialize, de::DeserializeOwned};
use std::future::Future;
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
use trillium::Status;
use trillium::{
    Conn,
    KnownHeaderName::{Accept, ContentType},
};

/// Extension trait that adds api methods to [`trillium::Conn`]
pub trait ApiConnExt {
    /// Sends a json response body. This sets a status code of 200,
    /// serializes the body with serde_json, sets the content-type to
    /// application/json, and [halts](trillium::Conn::halt) the
    /// conn. If serialization fails, a 500 status code is sent as per
    /// [`trillium::conn_try`]
    ///
    ///
    /// ## Examples
    ///
    /// ```
    /// # if !cfg!(any(feature = "sonic-rs", feature = "serde_json")) { return }
    /// use trillium_api::{json, ApiConnExt};
    /// use trillium_testing::TestHandler;
    ///
    /// async fn handler(conn: trillium::Conn) -> trillium::Conn {
    /// conn.with_json(&json!({ "json macro": "is reexported" }))
    /// }
    ///
    /// # trillium_testing::block_on(async {
    /// let app = TestHandler::new(handler).await;
    /// app.get("/")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body(r#"{"json macro":"is reexported"}"#)
    ///     .assert_header("content-type", "application/json");
    /// # });
    /// ```
    ///
    /// ### overriding status code
    /// ```
    /// use serde::Serialize;
    /// use trillium_api::ApiConnExt;
    /// use trillium_testing::TestHandler;
    ///
    /// #[derive(Serialize)]
    /// struct ApiResponse {
    ///     string: &'static str,
    ///     number: usize,
    /// }
    ///
    /// async fn handler(conn: trillium::Conn) -> trillium::Conn {
    ///     conn.with_json(&ApiResponse {
    ///         string: "not the most creative example",
    ///         number: 100,
    ///     })
    ///     .with_status(201)
    /// }
    ///
    /// # trillium_testing::block_on(async {
    /// let app = TestHandler::new(handler).await;
    /// app.get("/")
    ///     .await
    ///     .assert_status(201)
    ///     .assert_body(r#"{"string":"not the most creative example","number":100}"#)
    ///     .assert_header("content-type", "application/json");
    /// # });
    /// ```
    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    fn with_json(self, response: &impl Serialize) -> Self;

    /// Attempts to deserialize a type from the request body, based on the
    /// request content type.
    ///
    /// By default, both application/json and
    /// application/x-www-form-urlencoded are supported, and future
    /// versions may add accepted request content types. Please open an
    /// issue if you need to accept another content type.
    ///
    ///
    /// To exclusively accept application/json, disable default features
    /// on this crate.
    ///
    /// This sets a status code of Status::Ok if and only if no status
    /// code has been explicitly set.
    ///
    /// ## Examples
    ///
    /// ### Deserializing to `Value`
    ///
    /// ```no_run
    /// # if !cfg!(any(feature = "sonic-rs", feature = "serde_json")) { return }
    /// use trillium_api::{ApiConnExt, Value};
    ///
    /// async fn handler(mut conn: trillium::Conn) -> trillium::Conn {
    ///     let value: Value = match conn.deserialize().await {
    ///         Ok(v) => v,
    ///         Err(_) => return conn.with_status(400),
    ///     };
    ///     conn.with_json(&value)
    /// }
    ///
    /// # use trillium_testing::TestHandler;
    /// # trillium_testing::block_on(async {
    /// let app = TestHandler::new(handler).await;
    /// app.post("/")
    ///     .with_body(r#"key=value"#)
    ///     .with_request_header("content-type", "application/x-www-form-urlencoded")
    ///     .await
    ///     .assert_ok()
    ///     .assert_body(r#"{"key":"value"}"#)
    ///     .assert_header("content-type", "application/json");
    /// # });
    /// ```
    ///
    /// ### Deserializing a concrete type
    ///
    /// ```
    /// use trillium_api::ApiConnExt;
    /// use trillium_testing::TestHandler;
    ///
    /// #[derive(serde::Deserialize)]
    /// struct KvPair {
    ///     key: String,
    ///     value: String,
    /// }
    ///
    /// async fn handler(mut conn: trillium::Conn) -> trillium::Conn {
    ///     match conn.deserialize().await {
    ///         Ok(KvPair { key, value }) => conn
    ///             .with_status(201)
    ///             .with_body(format!("{} is {}", key, value))
    ///             .halt(),
    ///
    ///         Err(_) => conn.with_status(422).with_body("nope").halt(),
    ///     }
    /// }
    ///
    /// # trillium_testing::block_on(async {
    /// let app = TestHandler::new(handler).await;
    ///
    /// app.post("/")
    ///     .with_body(r#"key=name&value=trillium"#)
    ///     .with_request_header("content-type", "application/x-www-form-urlencoded")
    ///     .await
    ///     .assert_status(201)
    ///     .assert_body(r#"name is trillium"#);
    ///
    /// app.post("/")
    ///     .with_body(r#"name=trillium"#)
    ///     .with_request_header("content-type", "application/x-www-form-urlencoded")
    ///     .await
    ///     .assert_status(422)
    ///     .assert_body(r#"nope"#);
    /// # });
    /// ```
    fn deserialize<T>(&mut self) -> impl Future<Output = Result<T>> + Send
    where
        T: DeserializeOwned;

    /// Deserializes json without any Accepts header content negotiation
    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    fn deserialize_json<T>(&mut self) -> impl Future<Output = Result<T>> + Send
    where
        T: DeserializeOwned;

    /// Serializes the provided body using Accepts header content negotiation
    fn serialize<T>(&mut self, body: &T) -> impl Future<Output = Result<()>> + Send
    where
        T: Serialize + Sync;

    /// Returns a parsed content type for this conn.
    ///
    /// Note that this function considers a missing content type an error of variant
    /// [`Error::MissingContentType`].
    fn content_type(&self) -> Result<Mime>;
}

impl ApiConnExt for Conn {
    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    fn with_json(mut self, response: &impl Serialize) -> Self {
        #[cfg(feature = "serde_json")]
        let as_string = serde_json::to_string(&response);

        #[cfg(feature = "sonic-rs")]
        let as_string = sonic_rs::to_string(&response);

        match as_string {
            Ok(body) => {
                if self.status().is_none() {
                    self.set_status(Status::Ok);
                }

                self.response_headers_mut()
                    .try_insert(ContentType, "application/json");

                self.with_body(body)
            }

            Err(error) => self.with_state(Error::from(error)),
        }
    }

    async fn deserialize<T>(&mut self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let body = self.request_body_string().await?;
        let content_type = self.content_type()?;
        let suffix_or_subtype = content_type
            .suffix()
            .unwrap_or_else(|| content_type.subtype())
            .as_str();
        match suffix_or_subtype {
            #[cfg(feature = "serde_json")]
            "json" => {
                let json_deserializer = &mut serde_json::Deserializer::from_str(&body);
                Ok(serde_path_to_error::deserialize::<_, T>(json_deserializer)?)
            }

            #[cfg(feature = "sonic-rs")]
            "json" => {
                let json_deserializer = &mut sonic_rs::serde::Deserializer::from_str(&body);
                Ok(serde_path_to_error::deserialize::<_, T>(json_deserializer)?)
            }

            #[cfg(feature = "forms")]
            "x-www-form-urlencoded" => {
                let body = form_urlencoded::parse(body.as_bytes());
                let deserializer = serde_urlencoded::Deserializer::new(body);
                Ok(serde_path_to_error::deserialize::<_, T>(deserializer)?)
            }

            _ => {
                drop(body);
                Err(Error::UnsupportedMimeType {
                    mime_type: content_type.to_string(),
                })
            }
        }
    }

    fn content_type(&self) -> Result<Mime> {
        let header_str = self
            .request_headers()
            .get_str(ContentType)
            .ok_or(Error::MissingContentType)?;

        header_str.parse().map_err(|_| Error::UnsupportedMimeType {
            mime_type: header_str.into(),
        })
    }

    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    async fn deserialize_json<T>(&mut self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let content_type = self.content_type()?;
        let suffix_or_subtype = content_type
            .suffix()
            .unwrap_or_else(|| content_type.subtype())
            .as_str();
        if suffix_or_subtype != "json" {
            return Err(Error::UnsupportedMimeType {
                mime_type: content_type.to_string(),
            });
        }

        log::debug!("extracting json");
        let body = self.request_body_string().await?;

        #[cfg(feature = "serde_json")]
        let json_deserializer = &mut serde_json::Deserializer::from_str(&body);

        #[cfg(feature = "sonic-rs")]
        let json_deserializer = &mut sonic_rs::serde::Deserializer::from_str(&body);

        Ok(serde_path_to_error::deserialize::<_, T>(json_deserializer)?)
    }

    async fn serialize<T>(&mut self, body: &T) -> Result<()>
    where
        T: Serialize + Sync,
    {
        let accept = self
            .request_headers()
            .get_str(Accept)
            .unwrap_or("*/*")
            .split(',')
            .map(|s| s.trim())
            .find_map(acceptable_mime_type);

        match accept {
            #[cfg(feature = "serde_json")]
            Some(AcceptableMime::Json) => {
                self.set_body(serde_json::to_string(body)?);
                self.insert_response_header(ContentType, "application/json");
                Ok(())
            }

            #[cfg(feature = "sonic-rs")]
            Some(AcceptableMime::Json) => {
                self.set_body(sonic_rs::to_string(body)?);
                self.insert_response_header(ContentType, "application/json");
                Ok(())
            }

            #[cfg(feature = "forms")]
            Some(AcceptableMime::Form) => {
                self.set_body(serde_urlencoded::to_string(body)?);
                self.insert_response_header(ContentType, "application/x-www-form-urlencoded");
                Ok(())
            }

            None => {
                let _ = body;
                Err(Error::FailureToNegotiateContent)
            }
        }
    }
}

enum AcceptableMime {
    #[cfg(any(feature = "sonic-rs", feature = "serde_json"))]
    Json,

    #[cfg(feature = "forms")]
    Form,
}

fn acceptable_mime_type(mime: &str) -> Option<AcceptableMime> {
    let mime: Mime = mime.parse().ok()?;
    let suffix_or_subtype = mime.suffix().unwrap_or_else(|| mime.subtype()).as_str();
    match suffix_or_subtype {
        #[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
        "*" | "json" => Some(AcceptableMime::Json),

        #[cfg(feature = "forms")]
        "x-www-form-urlencoded" => Some(AcceptableMime::Form),

        _ => None,
    }
}
