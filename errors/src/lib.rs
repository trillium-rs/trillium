#[macro_export]
macro_rules! t {
    ($grain:expr) => {
        |mut conn: Conn| async move {
            let f = $grain;
            let r: std::result::Result<_, Box<dyn myco::Grain>> = f(&mut conn).await;
            match r {
                Ok(b) => conn.body(b),
                Err(e) => e.run(conn).await,
            }
        }
    };
}
