#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!

# Example

```
use trillium::{async_trait, Conn};
use trillium_controllers::{Controller, ControllerHandler};

struct UserController;

#[async_trait]
impl Controller for UserController {
  type Error = &'static str;
  async fn get(&self, conn: &mut Conn) -> Result<(), Self::Error> {
    conn.set_status(200);
    conn.set_body("ok");
    conn.set_halted(true);
    Ok(())
  }

  async fn delete(&self, conn: &mut Conn) -> Result<(), Self::Error> {
    Err("uh oh")
  }
}

let handler = ControllerHandler::new(UserController);
use trillium_testing::prelude::*;
assert_ok!(get("/").on(&handler), "ok");
assert_not_handled!(post("/").on(&handler));
assert_response!(delete("/").on(&handler), 500);
```

*/

use trillium::{async_trait, conn_try, http_types::Method, Conn, Handler};

#[async_trait]
pub trait Controller: Send + Sync + 'static {
    type Error: std::fmt::Display + Send + Sync + 'static;
    async fn get(&self, _conn: &mut Conn) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn post(&self, _conn: &mut Conn) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn put(&self, _conn: &mut Conn) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn delete(&self, _conn: &mut Conn) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn patch(&self, _conn: &mut Conn) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub struct ControllerHandler<C>(C);

impl<C> ControllerHandler<C> {
    pub fn new(controller: C) -> Self {
        Self(controller)
    }
}
#[async_trait]
impl<C: Controller> Handler for ControllerHandler<C> {
    async fn run(&self, mut conn: Conn) -> Conn {
        let result = match *conn.method() {
            Method::Get => self.0.get(&mut conn).await,
            Method::Post => self.0.post(&mut conn).await,
            Method::Put => self.0.put(&mut conn).await,
            Method::Delete => self.0.delete(&mut conn).await,
            Method::Patch => self.0.patch(&mut conn).await,
            _ => Ok(()),
        };

        conn_try!(conn, result);

        conn
    }
}
