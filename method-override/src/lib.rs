//! # Trillium method override handler
//!
//! This allows http clients that are unable to generate http methods
//! other than `GET` and `POST` to use `POST` requests that are
//! interpreted as other methods such as `PUT`, `PATCH`, or `DELETE`.
//!
//! This is currently supported with a querystring parameter of
//! `_method`. To change the querystring parameter's name, use
//! [`MethodOverride::with_param_name`]
//!
//! By default, the only methods allowed are `PUT`, `PATCH`, and
//! `DELETE`. To override this, use
//! [`MethodOverride::with_allowed_methods`]
//!
//! Subsequent handlers see the requested method on the conn instead of
//! POST.
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use querystrong::{IndexPath, QueryStrong};
use std::{collections::HashSet, fmt::Debug};
use trillium::{Conn, Handler, Method, Transport};

/// Trillium method override handler
///
/// See crate-level docs for an explanation
#[derive(Clone, Debug)]
pub struct MethodOverride {
    param: IndexPath<'static>,
    allowed_methods: HashSet<Method>,
}

impl Default for MethodOverride {
    fn default() -> Self {
        Self {
            param: IndexPath::parse("_method").unwrap(),
            allowed_methods: HashSet::from_iter([Method::Put, Method::Patch, Method::Delete]),
        }
    }
}

impl MethodOverride {
    /// constructs a new MethodOverride handler with default allowed methods and param name
    pub fn new() -> Self {
        Self::default()
    }

    /// replace the default allowed methods with the provided list of methods
    ///
    /// default: `put`, `patch`, `delete`
    ///
    /// ```
    /// # use trillium_method_override::MethodOverride;
    /// let handler = MethodOverride::new().with_allowed_methods(["put", "patch"]);
    /// ```
    pub fn with_allowed_methods<M>(mut self, methods: impl IntoIterator<Item = M>) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
    {
        self.allowed_methods = methods.into_iter().map(|m| m.try_into().unwrap()).collect();
        self
    }

    /// replace the default param name with the provided param name
    ///
    /// default: `_method`
    /// ```
    /// # use trillium_method_override::MethodOverride;
    /// let handler = MethodOverride::new().with_param_name("_http_method");
    /// ```
    pub fn with_param_name(mut self, param_name: &'static str) -> Self {
        self.param = IndexPath::parse(param_name).unwrap();
        self
    }
}

impl Handler for MethodOverride {
    async fn run(&self, mut conn: Conn) -> Conn {
        if conn.method() == Method::Post
            && let Some(method_str) =
                QueryStrong::parse(conn.querystring()).get_str(self.param.clone())
            && let Ok(method) = Method::try_from(method_str)
            && self.allowed_methods.contains(&method)
        {
            let mut_conn: &mut trillium_http::Conn<Box<dyn Transport>> = conn.as_mut();
            mut_conn.set_method(method);
        }

        conn
    }
}

/// Alias for MethodOverride::new()
pub fn method_override() -> MethodOverride {
    MethodOverride::new()
}
