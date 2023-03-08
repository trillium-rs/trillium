use crate::{ApiConnExt, Error, Extract};
use serde::{de::DeserializeOwned, Serialize};
use std::ops::{Deref, DerefMut};
use trillium::{async_trait, Conn, Handler, KnownHeaderName};

/// Body extractor
#[derive(Debug)]
pub struct Body<T>(pub T);

impl<T> Deref for Body<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Body<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl<T> Extract for Body<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    async fn extract(conn: &mut Conn) -> Option<Self> {
        match conn.deserialize::<T>().await {
            Ok(t) => Some(Self(t)),
            Err(e) => {
                conn.set_state(e);
                None
            }
        }
    }
}

enum AcceptableMime {
    Json,
    #[cfg(feature = "forms")]
    Form,
}

fn acceptable_mime_type(mime: &str) -> Option<AcceptableMime> {
    match mime {
        "*/*" | "application/json" => Some(AcceptableMime::Json),

        #[cfg(feature = "forms")]
        "application/x-www-form-urlencoded" => Some(AcceptableMime::Form),

        _ => None,
    }
}

fn negotiate_content_type<T>(conn: &mut Conn, body: &T) -> Result<(), Error>
where
    T: Serialize + Send + Sync + 'static,
{
    let accept = conn
        .headers()
        .get_str(KnownHeaderName::Accept)
        .unwrap_or("*/*")
        .split(',')
        .map(|s| s.trim())
        .find_map(acceptable_mime_type);

    match accept {
        Some(AcceptableMime::Json) => {
            conn.set_body(serde_json::to_string(&body)?);
            conn.headers_mut()
                .insert(KnownHeaderName::ContentType, "application/json");
            Ok(())
        }

        #[cfg(feature = "forms")]
        Some(AcceptableMime::Form) => {
            conn.set_body(serde_urlencoded::to_string(&body)?);
            conn.headers_mut().insert(
                KnownHeaderName::ContentType,
                "application/x-www-form-urlencoded",
            );
            Ok(())
        }

        None => Err(Error::FailureToNegotiateContent),
    }
}

#[async_trait]
impl<T> Handler for Body<T>
where
    T: Serialize + Send + Sync + 'static,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        match negotiate_content_type::<T>(&mut conn, &self.0) {
            Ok(()) => conn,
            Err(e) => conn.with_state(e),
        }
    }
}
