use crate::{Conn, Handler, Info};
use std::{future::Future, mem};

/// Provides support for asynchronous initialization of a handler after
/// the server is started.
///
/// ```
/// use trillium::{Conn, Init, State};
///
/// #[derive(Debug, Clone)]
/// struct MyDatabaseConnection(String);
/// impl MyDatabaseConnection {
///     async fn connect(uri: &str) -> std::io::Result<Self> {
///         Ok(Self(uri.into()))
///     }
///
///     async fn query(&self, query: &str) -> String {
///         format!("you queried `{}` against {}", query, &self.0)
///     }
/// }
///
/// let mut handler = (
///     Init::new(|mut info| async move {
///         let db = MyDatabaseConnection::connect("db://db").await.expect("1");
///         info.with_state(db)
///     }),
///     |conn: Conn| async move {
///         dbg!(&conn);
///         let db = conn.shared_state::<MyDatabaseConnection>().expect("2");
///         let response = db.query("select * from users limit 1").await;
///         conn.ok(response)
///     },
/// );
///
/// use trillium_testing::prelude::*;
///
/// block_on(async move {
///     let server_config = init(&mut handler).await;
///     assert_ok!(
///         get("/")
///             .with_server_config(server_config)
///             .run_async(&handler)
///             .await,
///         "you queried `select * from users limit 1` against db://db"
///     );
/// });
/// ```
#[derive(Debug)]
pub struct Init<F>(Option<F>);

impl<F, Fut> Init<F>
where
    F: FnOnce(Info) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Info> + Send + 'static,
{
    /// Constructs a new Init handler with an async function that receives and returns [`Info`].
    #[must_use]
    pub const fn new(init: F) -> Self {
        Self(Some(init))
    }
}

impl<F, Fut> Handler for Init<F>
where
    F: FnOnce(Info) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Info> + Send + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        conn
    }

    async fn init(&mut self, info: &mut Info) {
        if let Some(init) = self.0.take() {
            *info = init(mem::take(info)).await;
        } else {
            log::warn!("called init more than once");
        }
    }
}

/// alias for [`Init::new`]
pub const fn init<F, Fut>(init: F) -> Init<F>
where
    F: FnOnce(Info) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Info> + Send + 'static,
{
    Init::new(init)
}
