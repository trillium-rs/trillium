use crate::{Handler, Info};
use std::{any::type_name, borrow::Cow};

/// This handler populates a type into the immutable server-shared state type-set. Note that unlike
/// [`State`], this handler does not require [`Clone`], as the single allocation provided to the
/// constructor is held in an Arc and shared with every Conn.
///
#[derive(Debug)]
pub struct SharedState<T: Send + Sync + 'static>(Option<T>);
impl<T> SharedState<T>
where
    T: Send + Sync + 'static,
{
    /// Constructs a new State handler from any `Clone` + `Send` + `Sync` +
    /// `'static`
    pub const fn new(t: T) -> Self {
        Self(Some(t))
    }
}

/// Constructs a new [`SharedState`] handler from any Send + Sync +
/// 'static. Alias for [`SharedState::new`]
#[allow(clippy::missing_const_for_fn)]
pub fn shared_state<T: Send + Sync + 'static>(t: T) -> SharedState<T> {
    SharedState::new(t)
}

impl<T: Send + Sync + 'static> Handler for SharedState<T> {
    async fn init(&mut self, info: &mut Info) {
        info.insert_state(self.0.take().unwrap());
    }

    fn name(&self) -> Cow<'static, str> {
        format!("SharedState<{}>", type_name::<T>()).into()
    }
}
