use myco::{async_trait, http_types::Method, Conn, Grain};
use route_recognizer::{Match, Params, Router as MethodRouter};
use std::borrow::Cow;
use std::collections::HashMap;

pub trait RouterConnExt {
    fn param<'a>(&'a self, param: &str) -> Option<&'a str>;
}

impl RouterConnExt for Conn {
    fn param<'a>(&'a self, param: &str) -> Option<&'a str> {
        self.state::<Params>().and_then(|p| p.find(param))
    }
}

#[derive(Default)]
pub struct Router {
    method_map: HashMap<Method, MethodRouter<Box<dyn Grain>>>,
}

#[async_trait]
impl Grain for Router {
    async fn run(&self, mut conn: Conn) -> Conn {
        if let Some(m) = self.recognize(conn.method(), conn.path()) {
            conn.set_state(m.params().clone());
            m.handler().run(conn).await
        } else {
            conn
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        if let Some(m) = self.recognize(conn.method(), conn.path()) {
            m.handler().before_send(conn).await
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &myco::Upgrade) -> bool {
        if let Some(m) = self.recognize(upgrade.method(), upgrade.path()) {
            m.handler().has_upgrade(upgrade)
        } else {
            false
        }
    }

    async fn upgrade(&self, upgrade: myco::Upgrade) {
        self.recognize(upgrade.method(), upgrade.path())
            .unwrap()
            .handler()
            .upgrade(upgrade)
            .await
    }

    fn name(&self) -> Cow<'static, str> {
        "router (display tbd)".into()
    }
}

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(mut self, path: &str, grain: impl Grain) -> Self {
            self.add(path, Method::$method, grain);
            self
        }
    };
}

impl Router {
    pub fn new() -> Self {
        Router {
            method_map: HashMap::default(),
        }
    }

    #[allow(clippy::borrowed_box)] // this allow is because we don't have the ability to deref the
                                   // contents of the Match container. Clippy wants us to return
                                   // Option<Match<&dyn Grain>>, but route-recognizer would need
                                   // to support that
    pub fn recognize(&self, method: &Method, path: &str) -> Option<Match<&Box<dyn Grain>>> {
        self.method_map
            .get(method)
            .and_then(|r| r.recognize(path).ok())
    }

    pub fn add(&mut self, path: &str, method: Method, grain: impl Grain) {
        self.method_map
            .entry(method)
            .or_insert_with(MethodRouter::new)
            .add(path, Box::new(grain))
    }

    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);
}

#[macro_export]
macro_rules! routes {
    ($($method:ident $path:literal $(-> )?$grain:expr),+ $(,)?) => {
	$crate::Router::new()$(
            .$method($path, $grain)
        )+;
    };
}
