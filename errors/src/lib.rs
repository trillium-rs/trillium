#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
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
