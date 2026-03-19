use crate::{Conn, Handler, Info};
use std::{future::Future, mem};

/// Provides support for asynchronous initialization of a handler after
/// the server is started.
///
/// ```
/// use trillium::{Conn, Init, State};
/// use trillium_testing::TestHandler;
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
/// # trillium_testing::block_on(async {
/// let handler = (
///     Init::new(|mut info| async move {
///         let db = MyDatabaseConnection::connect("db://db").await.expect("1");
///         info.with_state(db)
///     }),
///     |conn: Conn| async move {
///         let db = conn.shared_state::<MyDatabaseConnection>().expect("2");
///         let response = db.query("select * from users limit 1").await;
///         conn.ok(response)
///     },
/// );
///
/// let app = TestHandler::new(handler).await;
/// app.get("/")
///     .await
///     .assert_ok()
///     .assert_body("you queried `select * from users limit 1` against db://db");
/// # });
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
        match self.0.take() {
            Some(init) => {
                *info = init(mem::take(info)).await;
            }
            _ => {
                log::warn!("called init more than once");
            }
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
