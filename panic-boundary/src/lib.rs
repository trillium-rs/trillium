use futures_lite::FutureExt;
use std::{
    borrow::Cow,
    panic::{resume_unwind, AssertUnwindSafe},
};
use trillium::{Body, Conn, Handler};
use trillium_macros::Handler;

pub struct DefaultPanicHandler;
#[trillium::async_trait]
impl Handler for DefaultPanicHandler {
    async fn run(&self, mut conn: Conn) -> Conn {
        let body = conn
            .take_panic_message()
            .map_or_else(|| "internal server error".into(), Body::from);
        conn.with_status(500).with_body(body).halt()
    }
}

#[derive(Handler, Debug)]
pub struct Unwind<H: Handler, PH: Handler> {
    #[handler(except = run)]
    handler: H,
    panic_handler: PH,
}

struct PanicMessage(Cow<'static, str>);

impl<H> Unwind<H, DefaultPanicHandler>
where
    H: Handler,
{
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            panic_handler: DefaultPanicHandler,
        }
    }
}

impl<H, PH> Unwind<H, PH>
where
    H: Handler,
    PH: Handler,
{
    pub fn with_panic_handler<PH2: Handler>(self, panic_handler: PH2) -> Unwind<H, PH2> {
        Unwind {
            handler: self.handler,
            panic_handler,
        }
    }

    pub async fn run(&self, mut conn: Conn) -> Conn {
        let (tx, rx) = async_channel::bounded(1);
        conn.on_drop(move |conn| {
            let _ = tx.try_send(conn);
        });

        match AssertUnwindSafe(self.handler.run(conn))
            .catch_unwind()
            .await
        {
            Ok(conn) => conn,
            Err(e) => match rx.recv().await {
                Ok(mut conn) => {
                    if let Some(s) = e.downcast_ref::<&str>() {
                        conn.set_state(PanicMessage(Cow::from(*s)));
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        conn.set_state(PanicMessage(Cow::from(s.clone())));
                    }

                    self.panic_handler.run(conn).await
                }

                Err(_) => resume_unwind(e),
            },
        }
    }
}

pub trait UnwindConnExt {
    fn panic_message(&self) -> Option<&str>;
    fn take_panic_message(&mut self) -> Option<Cow<'static, str>>;
}
impl UnwindConnExt for Conn {
    fn panic_message(&self) -> Option<&str> {
        self.state().map(|PanicMessage(ref cow)| &**cow)
    }

    fn take_panic_message(&mut self) -> Option<Cow<'static, str>> {
        self.take_state().map(|PanicMessage(cow)| cow)
    }
}
