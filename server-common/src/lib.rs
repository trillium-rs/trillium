use myco::{BoxedTransport, Conn, Grain, Transport};
use myco_http::Conn as HttpConn;

pub async fn handle_stream(stream: impl Transport, grain: impl Grain) {
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
                log::debug!("upgrading");
                grain.upgrade(upgrade).await;
            } else {
                log::error!("upgrade specified but no upgrade handler provided");
            }
        }

        Ok(None) => {
            log::info!("closing");
        }

        Err(e) => {
            log::error!("{:?}", e);
        }
    };
}
