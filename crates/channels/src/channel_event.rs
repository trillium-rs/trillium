use std::borrow::Cow;

use crate::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/**
# The messages passed between server and connected clients.

ChannelEvents contain a topic, event, payload, and if sent from a
client, a unique reference identifier that can be used to respond to
this event.

Most interfaces in this crate take an `Into<ChannelEvent>` instead of a ChannelEvent directly, so that you can either implement Into<ChannelEvent> for relevant types, or use these tuple From implementations:
```
use trillium_channels::ChannelEvent;
use serde_json::{json, Value, to_string};
let event: ChannelEvent = ("topic", "event").into();
assert_eq!(event.topic(), "topic");
assert_eq!(event.event(), "event");
assert_eq!(event.payload(), &json!({}));

let event: ChannelEvent = ("topic", "event", &json!({"some": "payload"})).into();
assert_eq!(event.topic(), "topic");
assert_eq!(event.event(), "event");
assert_eq!(to_string(event.payload()).unwrap(), r#"{"some":"payload"}"#);

#[derive(serde::Serialize)]
struct SomePayload { payload: &'static str };
let event: ChannelEvent = ("topic", "event", &SomePayload { payload: "anything" }).into();
assert_eq!(event.topic(), "topic");
assert_eq!(event.event(), "event");
assert_eq!(to_string(event.payload()).unwrap(), r#"{"payload":"anything"}"#);

*/

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChannelEvent {
    pub(crate) topic: Cow<'static, str>,
    pub(crate) event: Cow<'static, str>,
    pub(crate) payload: Value,

    #[serde(rename = "ref")]
    pub(crate) reference: Option<Cow<'static, str>>,

    #[serde(rename = "join_ref")]
    pub(crate) join_reference: Option<Cow<'static, str>>,
}

impl ChannelEvent {
    /**
    Construct a new ChannelEvent with the same reference as this
    ChannelEvent. Note that this is the only way of setting the
    reference on an event.

    The `event` argument can be either a `String` or, more commonly, a `&'static str`.

    The topic will always be the same as the source ChannelEvent's topic.
     */
    pub fn build_reply(
        &self,
        event: impl Into<Cow<'static, str>>,
        payload: &impl Serialize,
    ) -> ChannelEvent {
        ChannelEvent {
            topic: self.topic.clone(),
            event: event.into(),
            payload: match serde_json::to_value(payload).unwrap() {
                Value::Null => Value::Object(Default::default()),
                other => other,
            },
            reference: self.reference.clone(),
            join_reference: self.join_reference.clone(),
        }
    }

    pub(crate) fn serialize(&self, version: Version) -> serde_json::Result<String> {
        match version {
            Version::V1 => serde_json::to_string(&self),

            Version::V2 => serde_json::to_string(&(
                &self.join_reference,
                &self.reference,
                &self.topic,
                &self.event,
                &self.payload,
            )),
        }
    }

    pub(crate) fn deserialize(string: &str, version: Version) -> serde_json::Result<Self> {
        match version {
            Version::V1 => serde_json::from_str(string),
            Version::V2 => {
                let (join_reference, reference, topic, event, payload): (
                    Option<String>,
                    Option<String>,
                    String,
                    String,
                    Value,
                ) = serde_json::from_str(string)?;
                Ok(Self {
                    join_reference: join_reference.map(Into::into),
                    reference: reference.map(Into::into),
                    topic: topic.into(),
                    event: event.into(),
                    payload,
                })
            }
        }
    }

    /**
    Returns this ChannelEvent's topic
    */
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /**
    Returns this ChannelEvent's event
    */
    pub fn event(&self) -> &str {
        &self.event
    }

    /**
    Returns this ChannelEvent's payload as a Value
    */
    pub fn payload(&self) -> &Value {
        &self.payload
    }

    /**
    Returns the reference field ("ref" in json) for this ChannelEvent,
    if one was provided by the client
     */
    pub fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }

    /**
    Constructs a new ChannelEvent from topic, event, and a
    serializable payload. Use &() if no payload is needed.

    Note that the reference cannot be set this way. To set a
    reference, use [`ChannelEvent::build_reply`]
     */
    pub fn new(
        topic: impl Into<Cow<'static, str>>,
        event: impl Into<Cow<'static, str>>,
        payload: &impl Serialize,
    ) -> Self {
        Self {
            topic: topic.into(),
            event: event.into(),
            payload: match serde_json::to_value(payload).unwrap() {
                Value::Null => Value::Object(Default::default()),
                other => other,
            },
            reference: None,
            join_reference: None,
        }
    }

    /**
    returns true if this ChannelEvent is used by the phoenix-channels compatability layer

    currently that means the topic is `"phoenix"` or the event is `"phx_join"` or `"phx_leave"`
    */
    pub(crate) fn is_system_event(&self) -> bool {
        self.topic == "phoenix" || self.event == "phx_join" || self.event == "phx_leave"
    }
}

impl<T, E> From<(T, E)> for ChannelEvent
where
    T: Into<Cow<'static, str>>,
    E: Into<Cow<'static, str>>,
{
    fn from(te: (T, E)) -> Self {
        let (topic, event) = te;
        Self::new(topic, event, &())
    }
}

impl<T, E, P> From<(T, E, P)> for ChannelEvent
where
    T: Into<Cow<'static, str>>,
    E: Into<Cow<'static, str>>,
    P: Serialize,
{
    fn from(tep: (T, E, P)) -> Self {
        let (topic, event, payload) = tep;
        Self::new(topic, event, &payload)
    }
}
