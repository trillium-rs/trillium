use crate::{ChannelClient, ChannelEvent};
use serde::Serialize;
use serde_json::Value;
use trillium::TypeSet;
macro_rules! unwrap_or_log_and_return {
    ($expr:expr) => {
        match $expr {
            Some(value) => value,
            None => {
                log::error!(concat!("could not unwrap ", stringify!($expr)));
                return;
            }
        }
    };
}

pub trait ChannelConnExt {
    fn channel_client(&self) -> Option<&ChannelClient>;

    fn broadcast(&self, event: impl Into<ChannelEvent>) {
        unwrap_or_log_and_return!(self.channel_client()).broadcast(event);
    }

    fn send_event(
        &self,
        event: impl Into<ChannelEvent> + Send + '_,
    ) -> impl Future<Output = ()> + Send {
        async {
            unwrap_or_log_and_return!(self.channel_client())
                .send_event(event)
                .await;
        }
    }

    fn reply_ok(
        &self,
        event: &ChannelEvent,
        response: &(impl Serialize + Sync),
    ) -> impl Future<Output = ()> + Send {
        async {
            unwrap_or_log_and_return!(self.channel_client())
                .reply_ok(event, response)
                .await;
        }
    }

    fn reply_error(
        &self,
        event: &ChannelEvent,
        error: &(impl Serialize + Sync),
    ) -> impl Future<Output = ()> + Send {
        async {
            unwrap_or_log_and_return!(self.channel_client())
                .reply_error(event, error)
                .await;
        }
    }

    fn join(&self, event: &ChannelEvent, value: Option<Value>) -> impl Future<Output = ()> + Send {
        async {
            unwrap_or_log_and_return!(self.channel_client())
                .join(event, value)
                .await;
        }
    }

    fn leave(&self, event: &ChannelEvent, value: Option<Value>) -> impl Future<Output = ()> + Send {
        async {
            unwrap_or_log_and_return!(self.channel_client())
                .leave(event, value)
                .await;
        }
    }
}

impl<Conn> ChannelConnExt for Conn
where
    Conn: AsRef<TypeSet>,
{
    fn channel_client(&self) -> Option<&ChannelClient> {
        self.as_ref().get()
    }
}
