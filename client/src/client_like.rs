use crate::{Client, Conn};
use trillium_http::Method;
use trillium_server_common::{Connector, ObjectSafeConnector, Url};

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        method!(
            $fn_name,
            $method,
            concat!(
                "Builds a new client conn with the ",
                stringify!($fn_name),
                " http method and the provided url.
"
            )
        );
    };

    ($fn_name:ident, $method:ident, $doc_comment:expr) => {
        #[doc = $doc_comment]
        fn $fn_name(&self, url: Url) -> Conn
        where
            Self: Sized,
        {
            self.build_conn(Method::$method, url)
        }
    };
}

/// Trait for things that operate like a client. The only interface that's required is build_conn.
///
pub trait ClientLike {
    /// constructs a conn for the specified method and url.
    fn build_conn(&self, method: Method, url: Url) -> Conn;
    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);
}

impl<C: Connector + Clone> ClientLike for C {
    fn build_conn(&self, method: Method, url: Url) -> Conn {
        let client = Client::new(self.clone().arced());
        Conn::new_with_client(client, method, url)
    }
}
