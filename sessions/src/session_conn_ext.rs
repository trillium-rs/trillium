use crate::SessionStoreError;
use async_session::{Session, serde::Serialize};
use trillium::Conn;

/// extension trait to add session support to [`Conn`]
///
/// [`SessionHandler`](crate::SessionHandler) **MUST** be called on the
/// conn prior to using any of these functions.
pub trait SessionConnExt {
    /// append a key-value pair to the current session, where the key is a
    /// &str and the value is anything serde-serializable.
    fn with_session(self, key: &str, value: impl Serialize) -> Self;

    /// retrieve a reference to the current session
    fn session(&self) -> &Session;

    /// retrieve a mutable reference to the current session
    fn session_mut(&mut self) -> &mut Session;

    /// retrieve the error returned by the session store, if this request's session could not be
    /// loaded
    ///
    /// This is only ever present when the handler provided to
    /// [`SessionHandler::with_store_error_handler`](crate::SessionHandler::with_store_error_handler)
    /// did not halt.
    fn session_store_error(&self) -> Option<&SessionStoreError>;
}

impl SessionConnExt for Conn {
    fn session(&self) -> &Session {
        self.state()
            .expect("SessionHandler must be executed before calling SessionConnExt::sessions")
    }

    fn with_session(mut self, key: &str, value: impl Serialize) -> Self {
        self.session_mut().insert(key, value).ok();
        self
    }

    fn session_mut(&mut self) -> &mut Session {
        self.state_mut()
            .expect("SessionHandler must be executed before calling SessionConnExt::sessions_mut")
    }

    fn session_store_error(&self) -> Option<&SessionStoreError> {
        self.state()
    }
}
