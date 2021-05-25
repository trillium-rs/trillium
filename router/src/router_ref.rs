use crate::Router;
use trillium::{http_types::Method, Handler};

macro_rules! method_ref {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(&mut self, path: &'static str, handler: impl Handler) {
            self.0.add(path, Method::$method, handler);
        }
    };
}

#[derive(Debug)]
pub struct RouterRef<'r>(&'r mut Router);
impl<'r> RouterRef<'r> {
    method_ref!(get, Get);
    method_ref!(post, Post);
    method_ref!(put, Put);
    method_ref!(delete, Delete);
    method_ref!(patch, Patch);

    pub fn any(&mut self, path: &'static str, handler: impl Handler) {
        self.0.add_any(path, handler)
    }

    pub(crate) fn new(router: &'r mut Router) -> Self {
        Self(router)
    }
}
