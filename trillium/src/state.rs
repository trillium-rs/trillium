use std::fmt::{self, Debug};

use fmt::Formatter;

use crate::{async_trait, Conn, Handler};
/**
State is a handler that puts a clone of any `Clone + Send + Sync + 'static` type into every conn's state map.

```
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use trillium::{Conn, sequence, State};

#[derive(Clone, Default)] // Clone is mandatory
struct MyFeatureFlag(Arc<AtomicBool>);

impl MyFeatureFlag {
    pub fn is_enabled(&self) -> bool {
       self.0.load(Ordering::Relaxed)
    }
}

trillium_testing::server::run(sequence![
    State::new(MyFeatureFlag::default()),
    |conn: Conn| async move {
      if conn.state::<MyFeatureFlag>().unwrap().is_enabled() {
          conn.ok("path a")
      } else {
          conn.ok("path b")
      }
    }
]);
```

please note that as with the above contrived example, if your state
needs to be mutable, you need to choose your own interior mutability
with whatever cross thread synchronization mechanisms are appropriate
for your application. There will be one clones of the contained T type
in memory for each http connection, and any locks should be held as
briefly as possible so as to minimize impact on other conns.

stability note: This is a common enough pattern that it currently
exists in the public api, but may be removed at some point for
simplicity.
*/

pub struct State<T>(T);

impl<T: Debug> Debug for State<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("State").field(&self.0).finish()
    }
}

impl<T: Default> Default for State<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> State<T> {
    pub fn new(t: T) -> Self {
        Self(t)
    }
}
#[async_trait]
impl<T: Clone + Send + Sync + 'static> Handler for State<T> {
    async fn run(&self, mut conn: Conn) -> Conn {
        conn.set_state(self.0.clone());
        conn
    }
}
