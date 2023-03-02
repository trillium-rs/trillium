use serde::Serialize;
use trillium::{async_trait, Conn, Handler};

use crate::ApiConnExt;

/// A newtype wrapper struct for any [`serde::Serialize`] type. Note
/// that this currently must own the serializable type.
#[derive(Debug)]
pub struct Json<T>(pub T);

#[async_trait]
impl<Serializable> Handler for Json<Serializable>
where
    Serializable: Serialize + Send + Sync + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_json(&self.0)
    }
}
