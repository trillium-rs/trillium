use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

use crate::ChannelEvent;

/**
A data structure that tracks what topics a given client is subscribed to.
*/
#[derive(Clone, Default, Debug)]
pub struct Subscriptions(Arc<RwLock<BTreeSet<String>>>);
impl Subscriptions {
    /**
    adds the provided topic to the set of subscriptions. please note
    that this is case sensitive .
     */
    pub fn join(&self, topic: String) {
        self.0.write().unwrap().insert(topic);
    }

    /**
    removes the provided topic to the set of subscriptions, if it was
    previously subscribed. please note that this is case sensitive
     */
    pub fn leave(&self, topic: &str) {
        self.0.write().unwrap().remove(topic);
    }

    /**
    predicate function to determine if a ChannelEvent is applicable to
    a given user. `phx_join` and `phx_leave` are always applicable, as
    are any topics that are subscribed to by this client (as an exact
    match).
     */
    pub fn subscribes(&self, event: &ChannelEvent) -> bool {
        event.is_system_event() || self.0.read().unwrap().contains(event.topic())
    }
}
