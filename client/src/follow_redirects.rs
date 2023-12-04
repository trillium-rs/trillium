use crate::{async_trait, client_handler::ClientHandler, Conn, Error, KnownHeaderName, Result};
use std::mem;
use url::{ParseError, Url};

#[derive(Debug, Default, Copy, Clone)]
pub struct FollowRedirects {
    _private: (),
}

impl FollowRedirects {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

#[derive(Default, Debug)]
pub struct RedirectHistory(Vec<Url>);

#[async_trait]
impl ClientHandler for FollowRedirects {
    async fn after(&self, conn: &mut Conn) -> Result<()> {
        let client = conn.client().clone();

        if !matches!(conn.status(), Some(status) if status.is_redirection())
            || !conn.method().is_safe()
        {
            return Ok(());
        }

        let Some(location) = conn.response_headers().get_str(KnownHeaderName::Location) else {
            return Ok(());
        };

        let url = match Url::parse(location) {
            Ok(url) => url,
            Err(ParseError::RelativeUrlWithoutBase) => conn
                .url()
                .join(location)
                .map_err(|e| Error::Other(e.to_string()))?,
            Err(other_err) => return Err(Error::Other(other_err.to_string())),
        };

        let mut new_conn = client.build_conn(conn.method(), url.clone());
        new_conn.request_headers().append_all(
            conn.request_headers()
                .clone()
                .without_header(KnownHeaderName::Host),
        );

        *new_conn.state_mut() = std::mem::take(conn.state_mut());
        let old_conn = std::mem::replace(conn, new_conn);
        old_conn.recycle().await;

        conn.state_mut()
            .get_or_insert_with(RedirectHistory::default)
            .0
            .push(url);

        (&mut *conn).await?;

        Ok(())
    }
}
