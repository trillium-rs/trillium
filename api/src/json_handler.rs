use std::ops::{Deref, DerefMut};

use serde::{de::DeserializeOwned, Serialize};
use trillium::{async_trait, Conn, Handler};

use crate::{ApiConnExt, Extract};

/// A newtype wrapper struct for any [`serde::Serialize`] type. Note
/// that this currently must own the serializable type.
#[derive(Debug)]
pub struct Json<T>(pub T);

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl<Serializable> Handler for Json<Serializable>
where
    Serializable: Serialize + Send + Sync + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_json(&self.0)
    }
}

async fn extract_json<T>(conn: &mut Conn) -> Result<T, crate::Error>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    log::debug!("extracting json");
    let body = conn.request_body_string().await?;
    let json_deserializer = &mut serde_json::Deserializer::from_str(&body);
    Ok(serde_path_to_error::deserialize::<_, T>(json_deserializer)?)
}

#[async_trait]
impl<T> Extract for Json<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    async fn extract(conn: &mut Conn) -> Option<Self> {
        match extract_json(conn).await {
            Ok(t) => Some(Self(t)),
            Err(e) => {
                conn.set_state(e);
                None
            }
        }
    }
}
