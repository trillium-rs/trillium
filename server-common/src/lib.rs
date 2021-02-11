use myco::{BoxedTransport, Conn, Handler, Transport};
use myco_http::Conn as HttpConn;
pub use myco_tls_common::Acceptor;

pub async fn handle_stream<T: Transport>(
    stream: T,
    acceptor: impl Acceptor<T>,
    handler: impl Handler,
) {
    let stream = match acceptor.accept(stream).await {
        Ok(stream) => stream,
        Err(e) => {
            log::error!("acceptor error: {:?}", e);
            return;
        }
    };

    let result = HttpConn::map(stream, |conn| async {
        let conn = Conn::new(conn);
        let conn = handler.run(conn).await;
        let conn = handler.before_send(conn).await;
        conn.into_inner()
    })
    .await;

    match result {
        Ok(Some(upgrade)) => {
            let upgrade = upgrade.map_transport(BoxedTransport::new);
            if handler.has_upgrade(&upgrade) {
                log::debug!("upgrading...");
                handler.upgrade(upgrade).await;
            } else {
                log::error!("upgrade specified but no upgrade handler provided");
            }
        }

        Ok(None) => {
            log::debug!("closing connection");
        }

        Err(e) => {
            log::error!("http error: {:?}", e);
        }
    };
}
