#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
/*!
Basic authentication for trillium.rs

```rust,no_run
use trillium_basic_auth::BasicAuth;
trillium_smol::run((
    BasicAuth::new("trillium", "7r1ll1um").with_realm("rust"),
    |conn: trillium::Conn| async move { conn.ok("authenticated") },
));
```
*/
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use trillium::{
    async_trait, Conn, Handler,
    KnownHeaderName::{Authorization, WwwAuthenticate},
    Status,
};

/// basic auth handler
#[derive(Clone, Debug)]
pub struct BasicAuth {
    credentials: Credentials,
    realm: Option<String>,

    // precomputed/derived data fields:
    expected_header: String,
    www_authenticate: String,
}

/// basic auth username-password credentials
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    username: String,
    password: String,
}

impl Credentials {
    fn new(username: &str, password: &str) -> Self {
        Self {
            username: String::from(username),
            password: String::from(password),
        }
    }

    fn expected_header(&self) -> String {
        format!(
            "Basic {}",
            BASE64.encode(format!("{}:{}", self.username, self.password))
        )
    }

    // const BASIC: &str = "Basic ";
    // pub fn for_conn(conn: &Conn) -> Option<Self> {
    //     conn.request_headers()
    //         .get_str(KnownHeaderName::Authorization)
    //         .and_then(|value| {
    //             if value[..BASIC.len().min(value.len())].eq_ignore_ascii_case(BASIC) {
    //                 Some(&value[BASIC.len()..])
    //             } else {
    //                 None
    //             }
    //         })
    //         .and_then(|base64_credentials| BASE64.decode(base64_credentials).ok())
    //         .and_then(|credential_bytes| String::from_utf8(credential_bytes).ok())
    //         .and_then(|mut credential_string| {
    //             credential_string.find(":").map(|colon| {
    //                 let password = credential_string.split_off(colon + 1).into();
    //                 credential_string.pop();
    //                 Self {
    //                     username: credential_string.into(),
    //                     password,
    //                 }
    //             })
    //         })
    // }
}

impl BasicAuth {
    /// build a new basic auth handler with the provided username and password
    pub fn new(username: &str, password: &str) -> Self {
        let credentials = Credentials::new(username, password);
        let expected_header = credentials.expected_header();
        let realm = None;
        Self {
            expected_header,
            credentials,
            realm,
            www_authenticate: String::from("Basic"),
        }
    }

    /// provide a realm for the www-authenticate response sent by this handler
    pub fn with_realm(mut self, realm: &str) -> Self {
        self.www_authenticate = format!("Basic realm=\"{}\"", realm.replace('\"', "\\\""));
        self.realm = Some(String::from(realm));
        self
    }

    fn is_allowed(&self, conn: &Conn) -> bool {
        conn.request_headers().get_str(Authorization) == Some(&*self.expected_header)
    }

    fn deny(&self, conn: Conn) -> Conn {
        conn.with_status(Status::Unauthorized)
            .with_response_header(WwwAuthenticate, self.www_authenticate.clone())
            .halt()
    }
}

#[async_trait]
impl Handler for BasicAuth {
    async fn run(&self, conn: Conn) -> Conn {
        if self.is_allowed(&conn) {
            conn.with_state(self.credentials.clone())
        } else {
            self.deny(conn)
        }
    }
}
