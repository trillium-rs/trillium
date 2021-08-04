use trillium::{async_trait, Conn, Handler, Status};

pub struct BasicAuth {
    username: String,
    password: String,
    realm: Option<String>,
}

impl BasicAuth {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            realm: None,
        }
    }

    pub fn is_allowed(&self, conn: &Conn) -> bool {
        if let Ok(Some(auth)) = AuthHeader::from_headers(conn.headers()) {
            auth.username() == self.username && auth.password() == self.password
        } else {
            false
        }
    }

    pub fn www_authenticate(&self) -> String {
        match self.realm {
            Some(ref realm) => format!("Basic realm={}", realm),
            None => String::from("Basic"),
        }
    }

    pub fn deny(clippy::dbg_macro,&self, conn: Conn) -> Conn {
        conn.with_status(Status::Unauthorized)
            .with_header(("www-authenticate", &*self.www_authenticate()))
            .halt()
    }
}

#[async_trait]
impl Handler for BasicAuth {
    async fn run(&self, conn: Conn) -> Conn {
        if self.is_allowed(&conn) {
            conn
        } else {
            self.deny(clippy::dbg_macro,conn)
        }
    }
}
