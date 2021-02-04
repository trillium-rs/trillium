use myco::{BoxedTransport, Conn, Grain, Transport};
use myco_http::Conn as HttpConn;
pub use myco_tls_common::Acceptor;

pub async fn handle_stream<T: Transport>(stream: T, acceptor: impl Acceptor<T>, grain: impl Grain) {
    let stream = match acceptor.accept(stream).await {
        Ok(stream) => stream,
        Err(e) => {
            log::error!("acceptor error: {:?}", e);
            return;
        }
    };

    let result = HttpConn::map(stream, |conn| async {
        let conn = Conn::new(conn);
        let conn = grain.run(conn).await;
        let conn = grain.before_send(conn).await;
        conn.into_inner()
    })
    .await;

    match result {
        Ok(Some(upgrade)) => {
            let upgrade = upgrade.map_transport(BoxedTransport::new);
            if grain.has_upgrade(&upgrade) {
                log::debug!("upgrading...");
                grain.upgrade(upgrade).await;
            } else {
                log::error!("upgrade specified but no upgrade handler provided");
            }
        }

        Ok(None) => {
            log::info!("closing connection");
        }

        Err(e) => {
            log::error!("http error: {:?}", e);
        }
    };
}
