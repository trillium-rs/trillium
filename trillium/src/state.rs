use crate::{Conn, Handler};
use std::fmt::{self, Debug, Formatter};

/// # A handler for sharing state across an application.
///
/// State is a handler that puts a clone of any `Clone + Send + Sync +
/// 'static` type into every conn's state map.
///
/// ```
/// use std::sync::{
///     Arc,
///     atomic::{AtomicBool, Ordering},
/// };
/// use trillium::{Conn, State};
/// use trillium_testing::prelude::*;
///
/// #[derive(Clone, Default)] // Clone is mandatory
/// struct MyFeatureFlag(Arc<AtomicBool>);
///
/// impl MyFeatureFlag {
///     pub fn is_enabled(&self) -> bool {
///         self.0.load(Ordering::Relaxed)
///     }
///
///     pub fn toggle(&self) {
///         self.0.fetch_xor(true, Ordering::Relaxed);
///     }
/// }
///
/// let feature_flag = MyFeatureFlag::default();
///
/// let handler = (State::new(feature_flag.clone()), |conn: Conn| async move {
///     if conn.state::<MyFeatureFlag>().unwrap().is_enabled() {
///         conn.ok("feature enabled")
///     } else {
///         conn.ok("not enabled")
///     }
/// });
///
/// assert!(!feature_flag.is_enabled());
/// assert_ok!(get("/").on(&handler), "not enabled");
/// assert_ok!(get("/").on(&handler), "not enabled");
/// feature_flag.toggle();
/// assert!(feature_flag.is_enabled());
/// assert_ok!(get("/").on(&handler), "feature enabled");
/// assert_ok!(get("/").on(&handler), "feature enabled");
/// ```
///
/// Please note that as with the above contrived example, if your state
/// needs to be mutable, you need to choose your own interior mutability
/// with whatever cross thread synchronization mechanisms are appropriate
/// for your application. There will be one clones of the contained T type
/// in memory for each http connection, and any locks should be held as
/// briefly as possible so as to minimize impact on other conns.
///
/// **Stability note:** This is a common enough pattern that it currently
/// exists in the public api, but may be removed at some point for
/// simplicity.

pub struct State<T>(T);

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
    /// Constructs a new State handler from any `Clone` + `Send` + `Sync` +
    /// `'static`
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(t: T) -> Self {
        Self(t)
    }
}

/// Constructs a new [`State`] handler from any Clone + Send + Sync +
/// 'static. Alias for [`State::new`]
#[allow(clippy::missing_const_for_fn)]
pub fn state<T: Clone + Send + Sync + 'static>(t: T) -> State<T> {
    State::new(t)
}

impl<T: Clone + Send + Sync + 'static> Handler for State<T> {
    async fn run(&self, mut conn: Conn) -> Conn {
        conn.insert_state(self.0.clone());
        conn
    }
}
