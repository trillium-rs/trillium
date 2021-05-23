use crate::{async_trait, Conn, Upgrade};
use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

/**
# The building block for Trillium applications.

## Concept
Many other frameworks have a notion of `middleware` and `endpoints`,
in which the model is that a request passes through a router and then
any number of middlewares, then a single endpoint that returns a
response, and then passes a response back through the middleware
stack.

Because a Trillium Conn represents both a request and response, there
is no distinction between middleware and endpoints, as all of these
can be thought of as `Fn(Conn) -> Future<Output = Conn>`.

## Implementing Handler The simplest handler is an async closure or
async fn that receives a Conn and returns a Conn, and Handler has a
blanket implementation for any such Fn.

```
// as a closure
trillium_testing::server::run(|conn: trillium::Conn| async move { conn.ok("trillium!") });
```

```
// as an async function
async fn handler(conn: trillium::Conn) -> trillium::Conn {
    conn.ok("trillium!")
}
trillium_testing::server::run(handler);
```

The simplest implementation of Handler for a named type looks like this:
```
pub struct MyHandler;
#[trillium::async_trait]
impl trillium::Handler for MyHandler {
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        conn
    }
}

trillium_testing::server::run(MyHandler);
```

**temporary note:** until rust has true async traits, implementing
handler requires the use of the async_trait macro, which is reexported
as `trillium::async_trait`.


## Advanced usage
Unfortunately, async_trait results in ugly documentation above, so
here is how the trait is actually defined in trillium code:
```
# use trillium::{Conn, Upgrade};
# use std::borrow::Cow;
#[trillium::async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn run(&self, conn: Conn) -> Conn;
    async fn init(&mut self); // optional
    async fn before_send(&self, conn: Conn); // optional
    fn has_upgrade(&self, _upgrade: &Upgrade) -> bool; // optional
    async fn upgrade(&self, _upgrade: Upgrade); // mandatory only if has_upgrade returns true
    fn name(&self) -> Cow<'static, str>; // optional
}
```
See each of the function definitions below for advance implementation.

For most application code and even trillium-packaged framework code,
`run` is the only trait function that needs to be implemented.
*/
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    /// Executes this handler, performing any modifications to the
    /// Conn that are desired.
    async fn run(&self, conn: Conn) -> Conn;

    /**
    Performes one-time async set up on a mutable borrow of the
    Handler before the server starts accepting requests. This
    allows a Handler to be defined in synchronous code but perform
    async setup such as establishing a database connection or
    fetching some state from an external source. This is optional,
    and chances are high that you do not need this.

    **stability note:** This may go away at some point. Please open an
    **issue if you have a use case which requires it.
    */
    async fn init(&mut self) {}

    /**
    Performs any final modifications to this conn after all handlers
    have been run. Although this is a slight deviation from the simple
    conn->conn->conn chain represented by most Handlers, it provides
    an easy way for libraries to effectively inject a second handler
    into a response chain. This is useful for loggers that need to
    record information both before and after other handlers have run,
    as well as database transaction handlers and similar library code.

    **❗IMPORTANT NOTE FOR LIBRARY AUTHORS:** Please note that this
    will run __whether or not the conn has was halted before
    [`Handler::run`] was called on a given conn__. This means that if
    you want to make your `before_send` callback conditional on
    whether `run` was called, you need to put a unit type into the
    conn's state and check for that.

    stability note: I don't love this for the exact reason that it
    breaks the simplicity of the conn->conn->model, but it is
    currently the best compromise between that simplicity and
    convenience for the application author, who should not have to add
    two Handlers to achieve an "around" effect.
    */
    async fn before_send(&self, conn: Conn) -> Conn {
        conn
    }

    /**
    predicate function answering the question of whether this Handler
    would like to take ownership of the negotiated Upgrade. If this
    returns true, you must implement [`Handler::upgrade`]. The first
    handler that responds true to this will receive ownership of the
    [`trillium::Upgrade`][crate::Upgrade] in a subsequent call to [`Handler::upgrade`]
    */
    fn has_upgrade(&self, _upgrade: &Upgrade) -> bool {
        false
    }

    /**
    This will only be called if the handler reponds true to
    [`Handler::has_upgrade`] and will only be called once for this
    upgrade. There is no return value, and this function takes
    exclusive ownership of the underlying transport once this is
    called. You can downcast the transport to whatever the source
    transport type is and perform any non-http protocol communication
    that has been negotiated. You probably don't want this unless
    you're implementing something like websockets. Please note that
    for many transports such as TcpStreams, dropping the transport
    (and therefore the Upgrade) will hang up / disconnect.
    */
    async fn upgrade(&self, _upgrade: Upgrade) {
        unimplemented!("if has_upgrade returns true, you must also implement upgrade")
    }

    /**
    Customize the name of your handler. This is used in Debug
    implementations. The default is the type name of this handler.
    */
    fn name(&self) -> Cow<'static, str> {
        std::any::type_name::<Self>().into()
    }
}

#[async_trait]
impl Handler for Box<dyn Handler> {
    async fn run(&self, conn: Conn) -> Conn {
        self.as_ref().run(conn).await
    }
    async fn init(&mut self) {
        self.as_mut().init().await
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.as_ref().before_send(conn).await
    }

    fn name(&self) -> Cow<'static, str> {
        self.as_ref().name()
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.as_ref().has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.as_ref().upgrade(upgrade).await
    }
}

impl std::fmt::Debug for Box<dyn Handler> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name().as_ref())
    }
}

#[async_trait]
impl<G: Handler> Handler for Arc<G> {
    async fn run(&self, conn: Conn) -> Conn {
        self.as_ref().run(conn).await
    }

    async fn init(&mut self) {
        Arc::<G>::get_mut(self)
            .expect("cannot call init when there are already clones of an Arc<Handler>")
            .init()
            .await
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.as_ref().before_send(conn).await
    }

    fn name(&self) -> Cow<'static, str> {
        self.as_ref().name()
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.as_ref().has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.as_ref().upgrade(upgrade).await
    }
}

#[async_trait]
impl<G: Handler> Handler for Vec<G> {
    async fn run(&self, mut conn: Conn) -> Conn {
        for handler in self {
            conn = handler.run(conn).await;
            if conn.is_halted() {
                break;
            }
        }
        conn
    }

    async fn init(&mut self) {
        for handler in self {
            handler.init().await;
        }
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        for handler in self.iter().rev() {
            conn = handler.before_send(conn).await
        }
        conn
    }

    fn name(&self) -> Cow<'static, str> {
        self.iter()
            .map(|v| v.name())
            .collect::<Vec<_>>()
            .join(",")
            .into()
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.iter().any(|g| g.has_upgrade(upgrade))
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        if let Some(handler) = self.iter().find(|g| g.has_upgrade(&upgrade)) {
            handler.upgrade(upgrade).await
        }
    }
}

#[async_trait]
impl<Fun, Fut> Handler for Fun
where
    Fun: Fn(Conn) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn> + Send + Sync + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        (self)(conn).await
    }
}

#[async_trait]
impl Handler for String {
    async fn run(&self, conn: Conn) -> Conn {
        conn.ok(&self[..])
    }
}

#[async_trait]
impl Handler for &'static str {
    async fn run(&self, conn: Conn) -> Conn {
        conn.ok(*self)
    }

    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(self)
    }
}

macro_rules! reverse_before_send {
    ($conn:ident, $name:ident) => (
        let $conn = ($name).before_send($conn).await;
    );

    ($conn:ident, $name:ident $($other_names:ident)+) => (
        reverse_before_send!($conn, $($other_names)*);
        reverse_before_send!($conn, $name);
    );
}

macro_rules! impl_handler_tuple {
        ($($name:ident)+) => (
            #[async_trait]
            impl<$($name),*> Handler for ($($name,)*) where $($name: Handler),* {
                #[allow(non_snake_case)]
                async fn run(&self, conn: Conn) -> Conn {
                    let ($(ref $name,)*) = *self;
                    $(
                        let conn = ($name).run(conn).await;
                        if conn.is_halted() { return conn }
                    )*
                    conn
                }

                #[allow(non_snake_case)]
                fn name(&self) -> Cow<'static, str> {
                    let ($(ref $name,)*) = *self;
                    format!("({})", [$(($name).name(),)*].join(", ")).into()
                }


                #[allow(non_snake_case)]
                async fn init(&mut self) {
                    let ($(ref mut $name,)*) = *self;
                    $(($name).init().await;)*
                }

                #[allow(non_snake_case)]
                async fn before_send(&self, conn: Conn) -> Conn {
                    let ($(ref $name,)*) = *self;
                    reverse_before_send!(conn, $($name)+);
                    conn
                }

                #[allow(non_snake_case)]
                fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
                    let ($(ref $name,)*) = *self;
                    $(if ($name).has_upgrade(upgrade) { return true })*
                    false
                }

                #[allow(non_snake_case)]
                async fn upgrade(&self, upgrade: Upgrade) {
                    let ($(ref $name,)*) = *self;
                    $(if ($name).has_upgrade(&upgrade) {
                        return ($name).upgrade(upgrade).await;
                    })*
                }
            }
        );
    }

impl_handler_tuple! { A B }
impl_handler_tuple! { A B C }
impl_handler_tuple! { A B C D }
impl_handler_tuple! { A B C D E }
impl_handler_tuple! { A B C D E F }
impl_handler_tuple! { A B C D E F G }
impl_handler_tuple! { A B C D E F G H }
impl_handler_tuple! { A B C D E F G H I }
impl_handler_tuple! { A B C D E F G H I J }
impl_handler_tuple! { A B C D E F G H I J K }
impl_handler_tuple! { A B C D E F G H I J K L }
impl_handler_tuple! { A B C D E F G H I J K L M }
impl_handler_tuple! { A B C D E F G H I J K L M N }
impl_handler_tuple! { A B C D E F G H I J K L M N O }
