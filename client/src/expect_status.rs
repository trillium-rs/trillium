use crate::{async_trait, ClientHandler, Conn, Error, Status};
use std::collections::HashSet;

/// Handler to treat unexpected status codes as errors
#[derive(Debug)]
pub struct ExpectStatus {
    expected_statuses: HashSet<Status>,
}

#[async_trait]
impl ClientHandler for ExpectStatus {
    async fn after(&self, conn: &mut Conn) -> crate::Result<()> {
        if conn
            .status()
            .map_or(false, |status| self.expected_statuses.contains(&status))
        {
            Ok(())
        } else {
            Err(Error::Other(format!(
                "unexpected status {:?}, expected {:?}",
                conn.status(),
                &self.expected_statuses
            )))
        }
    }
}

impl ExpectStatus {
    /// build a new status expectation handler
    pub fn new(statuses: impl IntoIterator<Item = Status>) -> Self {
        Self {
            expected_statuses: statuses.into_iter().collect(),
        }
    }

    /// expect http success (2xx)
    pub fn success() -> Self {
        Self::new([
            Status::Ok,
            Status::Created,
            Status::Accepted,
            Status::NonAuthoritativeInformation,
            Status::NoContent,
            Status::ResetContent,
            Status::PartialContent,
        ])
    }
}
