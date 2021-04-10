#[macro_export]
macro_rules! t {
    ($handler:expr) => {
        |mut conn: Conn| async move {
            let f = $handler;
            let r: std::result::Result<_, Box<dyn trillium::Handler>> = f(&mut conn).await;
            match r {
                Ok(b) => conn.body(b),
                Err(e) => e.run(conn).await,
            }
        }
    };
}
