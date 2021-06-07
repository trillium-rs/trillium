use std::future::Future;
use std::pin::Pin;
use trillium::{async_trait, Conn, Handler};

#[async_trait]
pub trait Errorable: Send {
    async fn run(self, conn: Conn) -> Conn;
}

pub type ErrorResult<'a, Errorable> =
    Pin<Box<dyn Future<Output = Result<(), Errorable>> + Send + 'a>>;

pub struct ErrorHandler<F>(F);

#[trillium::async_trait]
impl<F, E> Handler for ErrorHandler<F>
where
    F: for<'a> Fn(&'a mut Conn) -> Pin<Box<dyn Future<Output = Result<(), E>> + Send + 'a>>
        + Send
        + Sync
        + 'static,
    E: Errorable,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        match self.0(&mut conn).await {
            Ok(()) => conn,
            Err(h) => h.run(conn).await,
        }
    }
}

impl<F, E> ErrorHandler<F>
where
    F: for<'a> Fn(&'a mut Conn) -> Pin<Box<dyn Future<Output = Result<(), E>> + Send + 'a>>
        + Send
        + Sync
        + 'static,
    E: Errorable,
{
    pub fn new(handler_function: F) -> Self {
        Self(handler_function)
    }
}
