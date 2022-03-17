use crate::{async_trait, Conn, Handler, Info, Upgrade};
use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    future::Future,
    mem,
    ops::Deref,
    pin::Pin,
};

/**

Provides support for asynchronous initialization of a handler after
the server is started.

```
use trillium::{Conn, State, Init};

#[derive(Debug, Clone)]
struct MyDatabaseConnection(String);
impl MyDatabaseConnection {
    async fn connect(uri: String) -> std::io::Result<Self> {
        Ok(Self(uri))
    }
    async fn query(&mut self, query: &str) -> String {
        format!("you queried `{}` against {}", query, &self.0)
    }
}

let mut handler = (
    Init::new(|_| async {
        let url = std::env::var("DATABASE_URL").unwrap();
        let db = MyDatabaseConnection::connect(url).await.unwrap();
        State::new(db)
    }),
    |mut conn: Conn| async move {
        let db = conn.state_mut::<MyDatabaseConnection>().unwrap();
        let response = db.query("select * from users limit 1").await;
        conn.ok(response)
    }
);

std::env::set_var("DATABASE_URL", "db://db");

use trillium_testing::prelude::*;

init(&mut handler);
assert_ok!(
    get("/").on(&handler),
    "you queried `select * from users limit 1` against db://db"
);

```

Because () is the noop handler, this can also be used to perform one-time set up:
```
use trillium::{Init, Conn};

let mut handler = (
    Init::new(|info| async move { log::info!("{}", info); }),
    |conn: Conn| async move { conn.ok("ok!") }
);

use trillium_testing::prelude::*;
init(&mut handler);
assert_ok!(get("/").on(&handler), "ok!");
```
*/
pub struct Init<T>(Inner<T>);

type Initializer<T> =
    Box<dyn Fn(Info) -> Pin<Box<dyn Future<Output = T> + Send + 'static>> + Send + Sync + 'static>;

enum Inner<T> {
    New(Initializer<T>),
    Initializing,
    Initialized(T),
}

impl<T: Handler> Deref for Inner<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Inner::Initialized(t) => t,
            _ => {
                panic!("attempted to dereference uninitialized handler {:?}", &self);
            }
        }
    }
}

impl<T: Handler> Debug for Inner<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialized(ref t) => f.debug_tuple("Initialized").field(&t.name()).finish(),

            Self::New(_) => f
                .debug_tuple("New")
                .field(&std::any::type_name::<T>())
                .finish(),

            Self::Initializing => f
                .debug_tuple("Initializing")
                .field(&std::any::type_name::<T>())
                .finish(),
        }
    }
}

impl<T: Handler> Debug for Init<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Init").field(&self.0).finish()
    }
}

impl<T: Handler> Init<T> {
    /**
    Constructs a new Init handler with an async function that
    returns the handler post-initialization. The async function
    receives [`Info`] for the current server.
    */
    pub fn new<F, Fut>(init: F) -> Self
    where
        F: Fn(Info) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        Self(Inner::New(Box::new(move |info| Box::pin(init(info)))))
    }
}

#[async_trait]
impl<T: Handler> Handler for Init<T> {
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    async fn init(&mut self, info: &mut Info) {
        self.0 = match mem::replace(&mut self.0, Inner::Initializing) {
            Inner::New(init) => Inner::Initialized(init(info.clone()).await),
            other => other,
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.0.before_send(conn).await
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.0.has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.0.upgrade(upgrade).await;
    }

    fn name(&self) -> Cow<'static, str> {
        match &self.0 {
            Inner::New(_) => format!("uninitialized {}", std::any::type_name::<T>()).into(),
            Inner::Initializing => {
                format!("currently initializing {}", std::any::type_name::<T>()).into()
            }
            Inner::Initialized(t) => t.name(),
        }
    }
}
