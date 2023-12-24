use serde::Serialize;
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::HashMap,
    ops::{Deref, DerefMut},
};

/**
A struct for accumulating key-value data for use in handlebars
templates. The values can be any type that is serde serializable
*/
#[derive(Default, Serialize, Debug)]
pub struct Assigns(HashMap<Cow<'static, str>, Value>);

impl Deref for Assigns {
    type Target = HashMap<Cow<'static, str>, Value>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Assigns {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
