use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    mem,
    ops::Deref,
    pin::Pin,
};

use crate::{async_trait, Conn, Handler};
/**
# A handler for sharing state across an application.

State is a handler that puts a clone of any `Clone + Send + Sync +
'static` type into every conn's state map.

```
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use trillium::{Conn, State};
use trillium_testing::prelude::*;


#[derive(Clone, Default, Debug)] // Clone is mandatory
struct MyFeatureFlag(Arc<AtomicBool>);

impl MyFeatureFlag {
    pub fn is_enabled(&self) -> bool {
       self.0.load(Ordering::Relaxed)
    }

    pub fn toggle(&self) {
       self.0.fetch_xor(true, Ordering::Relaxed);
    }
}

let feature_flag = MyFeatureFlag::default();

let handler = (
    State::new(feature_flag.clone()),
    |conn: Conn| async move {
      if conn.state::<MyFeatureFlag>().unwrap().is_enabled() {
          conn.ok("feature enabled")
      } else {
          conn.ok("not enabled")
      }
    }
);

assert!(!feature_flag.is_enabled());
assert_ok!(get("/").on(&handler), "not enabled");
assert_ok!(get("/").on(&handler), "not enabled");
feature_flag.toggle();
assert!(feature_flag.is_enabled());
assert_ok!(get("/").on(&handler), "feature enabled");
assert_ok!(get("/").on(&handler), "feature enabled");

```

Please note that as with the above contrived example, if your state
needs to be mutable, you need to choose your own interior mutability
with whatever cross thread synchronization mechanisms are appropriate
for your application. There will be one clones of the contained T type
in memory for each http connection, and any locks should be held as
briefly as possible so as to minimize impact on other conns.

## Async initializer

If your state type needs to be initialized asynchronously, State also
provides a convenience utility `State::init`, which takes a future
that returns the type that will then be cloned and placed in each
conn's State set.

```
use trillium::{Conn, State, Handler};
use trillium_testing::prelude::*;

#[derive(Clone, Debug)]
struct MyDatabaseConnection;

impl MyDatabaseConnection {
    async fn connect(_uri: String) -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn query(&mut self, query: &str) -> String {
        format!("you queried: `{}`", query)
    }
}

let mut handler = (
    State::init(async {
        let database_url = std::env::var("DATABASE_URL").unwrap();
        MyDatabaseConnection::connect(database_url).await.unwrap()
    }),
    |mut conn: Conn| async move {
      let mut db = conn.state_mut::<MyDatabaseConnection>().unwrap();
      let response = db.query("select * from users where name = 'bobby'").await;
      conn.ok(response)
    }
);


std::env::set_var("DATABASE_URL", "this is just for demonstration purposes");

// this normally performed by the runtime adapter when not in test mode
trillium_testing::block_on(handler.init());

assert_ok!(get("/").on(&handler), "you queried: `select * from users where name = 'bobby'`");
```

# Stability note

This is a common enough pattern that it currently
exists in the public api, but may be removed at some point for
simplicity.
*/

pub struct State<T>(Inner<T>);

enum Inner<T> {
    New(Pin<Box<dyn Future<Output = T> + Send + Sync + 'static>>),
    Initializing,
    Initialized(T),
}

impl<T> Deref for Inner<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Inner::Initialized(t) => t,
            _ => unreachable!(),
        }
    }
}

impl<T: Debug> Debug for Inner<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialized(ref t) => f.debug_tuple("Initialized").field(&t).finish(),
            Self::New(_) => f.debug_tuple("New").finish(),
            Self::Initializing => f.debug_tuple("Initializing").finish(),
        }
    }
}

impl<T: Debug> Debug for State<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("State").field(&self.0).finish()
    }
}

impl<T> Default for State<T>
where
    T: Default + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> State<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Constructs a new State handler from any Clone + Send + Sync +
    /// 'static
    pub fn new(t: T) -> Self {
        Self(Inner::Initialized(t))
    }

    /// Constructs a new State handler from a future that returns the
    /// state type that will be cloned into every Conn's state set.
    pub fn init<Fut>(f: Fut) -> Self
    where
        Fut: Future<Output = T> + Send + Sync + 'static,
    {
        Self(Inner::New(Box::pin(f)))
    }
}

#[async_trait]
impl<T: Clone + Send + Sync + 'static> Handler for State<T> {
    async fn init(&mut self) {
        if let Inner::New(f) = mem::replace(&mut self.0, Inner::Initializing) {
            self.0 = Inner::Initialized(f.await);
        }
    }

    async fn run(&self, mut conn: Conn) -> Conn {
        conn.set_state(self.0.clone());
        conn
    }
}
