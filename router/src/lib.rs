use routefinder::{Captures, Match};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use trillium::{async_trait, http_types::Method, Conn, Handler};

pub trait RouterConnExt {
    fn param<'a>(&'a self, param: &str) -> Option<&'a str>;
    fn wildcard(&self) -> Option<&str>;
}

impl RouterConnExt for Conn {
    fn param<'a>(&'a self, param: &str) -> Option<&'a str> {
        self.state::<Captures>().and_then(|p| p.get(param))
    }

    fn wildcard(&self) -> Option<&str> {
        self.state::<Captures>().and_then(|p| p.wildcard())
    }
}

#[derive(Default, Debug)]
pub struct Router(HashMap<Method, routefinder::Router<Box<dyn Handler>>>);

#[async_trait]
impl Handler for Router {
    async fn run(&self, conn: Conn) -> Conn {
        if let Some(m) = self.best_match(conn.method(), conn.path()) {
            let captures = m.captures().into_owned();
            struct HasPath;
            log::debug!("running {}: {}", m.route(), m.name());
            let mut new_conn = m
                .handler()
                .run({
                    let mut conn = conn;
                    if let Some(wildcard) = captures.wildcard() {
                        conn.push_path(String::from(wildcard));
                        conn.set_state(HasPath);
                    }
                    conn.with_state(captures)
                })
                .await;
            if new_conn.take_state::<HasPath>().is_some() {
                new_conn.pop_path();
            }
            new_conn
        } else {
            log::debug!("{} did not match any route", conn.path());
            conn
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        if let Some(m) = self.best_match(conn.method(), conn.path()) {
            m.handler().before_send(conn).await
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &trillium::Upgrade) -> bool {
        if let Some(m) = self.best_match(upgrade.method(), upgrade.path()) {
            m.handler().has_upgrade(upgrade)
        } else {
            false
        }
    }

    async fn upgrade(&self, upgrade: trillium::Upgrade) {
        self.best_match(upgrade.method(), upgrade.path())
            .unwrap()
            .handler()
            .upgrade(upgrade)
            .await
    }
}

macro_rules! method_ref {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(&mut self, path: &'static str, handler: impl Handler) {
            self.0.add(path, Method::$method, handler);
        }
    };
}

pub struct RouterRef<'r>(&'r mut Router);
impl RouterRef<'_> {
    method_ref!(get, Get);
    method_ref!(post, Post);
    method_ref!(put, Put);
    method_ref!(delete, Delete);
    method_ref!(patch, Patch);

    pub fn any(&mut self, path: &'static str, handler: impl Handler) {
        self.0.register_any(path, handler)
    }
}

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(mut self, path: &'static str, handler: impl Handler) -> Self {
            self.add(path, Method::$method, handler);
            self
        }
    };
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(b: impl Fn(RouterRef)) -> Router {
        let mut router = Router::new();
        b(RouterRef(&mut router));
        router
    }

    pub fn best_match<'a, 'b>(
        &'a self,
        method: &Method,
        path: &'b str,
    ) -> Option<Match<'a, 'b, Box<dyn Handler>>> {
        self.0.get(method).and_then(|r| r.best_match(path))
    }

    pub fn add(&mut self, path: &'static str, method: Method, handler: impl Handler) {
        self.0
            .entry(method)
            .or_insert_with(routefinder::Router::new)
            .add(path, Box::new(handler))
            .expect("could not add route")
    }

    pub fn register_any(&mut self, path: &'static str, handler: impl Handler) {
        use Method::*;
        let handler = Arc::new(handler);
        for method in [Get, Post, Put, Delete, Patch] {
            self.add(path, method, handler.clone())
        }
    }

    pub fn any(mut self, path: &'static str, handler: impl Handler) -> Self {
        self.register_any(path, handler);
        self
    }

    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);
}

#[macro_export]
macro_rules! routes {
    ($($method:ident $path:literal $(-> )?$handler:expr),+ $(,)?) => {
	$crate::Router::new()$(
            .$method($path, $handler)
        )+;
    };
}
