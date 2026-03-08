use crate::{ArcHandler, QuicBinding, QuicConnection, Runtime};
use std::sync::Arc;
use trillium::{Conn, Handler};
use trillium_http::{
    ServerConfig,
    h3::{H3Connection, H3RequestError},
    transport::BoxedTransport,
};

pub(crate) async fn run_h3<QB: QuicBinding>(
    quic_binding: QB,
    server_config: Arc<ServerConfig>,
    handler: ArcHandler<impl Handler>,
    runtime: Runtime,
) {
    let swansong = server_config.swansong();
    while let Some(connection) = swansong.interrupt(quic_binding.accept()).await.flatten() {
        let h3 = H3Connection::new(server_config.clone());
        let handler = handler.clone();
        let runtime = runtime.clone();
        runtime
            .clone()
            .spawn(run_h3_connection(connection, h3, handler, runtime));
    }
}

async fn run_h3_connection<QC: QuicConnection>(
    connection: QC,
    h3: Arc<H3Connection>,
    handler: ArcHandler<impl Handler>,
    runtime: Runtime,
) {
    // Outbound control stream
    {
        let (connection, h3) = (connection.clone(), h3.clone());
        runtime.spawn(async move {
            let result: Result<(), H3RequestError> =
                async { h3.outbound_control(connection.open_uni().await?).await }.await;
            if let Err(error) = result {
                handle_h3_error(error, &connection, &h3).await;
            }
        });
    }

    // QPACK encoder stream
    {
        let (connection, h3) = (connection.clone(), h3.clone());
        runtime.spawn(async move {
            let result: Result<(), H3RequestError> =
                async { h3.encoder(connection.open_uni().await?).await }.await;
            if let Err(error) = result {
                handle_h3_error(error, &connection, &h3).await;
            }
        });
    }

    // QPACK decoder stream
    {
        let (connection, h3) = (connection.clone(), h3.clone());
        runtime.spawn(async move {
            let result: Result<(), H3RequestError> =
                async { h3.decoder(connection.open_uni().await?).await }.await;
            if let Err(error) = result {
                handle_h3_error(error, &connection, &h3).await;
            }
        });
    }

    // Inbound unidirectional streams
    {
        let (connection, h3, runtime) = (connection.clone(), h3.clone(), runtime.clone());
        runtime.clone().spawn(async move {
            while let Ok(recv) = connection.accept_uni().await {
                let (connection, h3) = (connection.clone(), h3.clone());
                runtime.spawn(async move {
                    let stop_connection = connection.clone();
                    let result = h3
                        .inbound_uni(recv, async move |stream, error_code| {
                            stop_connection.stop_stream(stream, error_code.into());
                        })
                        .await;
                    if let Err(error) = result {
                        handle_h3_error(error, &connection, &h3).await;
                    }
                });
            }
        });
    }

    // Bidirectional request streams
    let swansong = h3.swansong().clone();
    while let Some(Ok((stream_id, transport))) = swansong.interrupt(connection.accept_bi()).await {
        let (h3, handler, connection) = (h3.clone(), handler.clone(), connection.clone());
        let peer_ip = connection.remote_address().ip();
        runtime.spawn(async move {
            let handler = &handler;
            let quic_connection = connection.clone();
            let result = h3
                .clone()
                .run_request(
                    transport,
                    |mut conn| async move {
                        conn.set_peer_ip(Some(peer_ip));
                        conn.set_secure(true);
                        let conn = Conn::from(conn).with_state(quic_connection);
                        let conn = handler.run(conn).await;
                        let conn = handler.before_send(conn).await;
                        conn.into_inner()
                    },
                    stream_id,
                )
                .await;

            match result {
                Ok(conn) if conn.should_upgrade() => {
                    let upgrade =
                        trillium_http::Upgrade::from(conn).map_transport(BoxedTransport::new);
                    if handler.has_upgrade(&upgrade) {
                        log::debug!("upgrading h3 stream");
                        handler.upgrade(upgrade).await;
                    } else {
                        log::error!("h3 upgrade specified but no upgrade handler provided");
                    }
                }
                Ok(_) => {}
                Err(error) => handle_h3_error(error, &connection, &h3).await,
            }
        });
    }
}

async fn handle_h3_error(
    error: H3RequestError,
    connection: &impl QuicConnection,
    h3: &H3Connection,
) {
    log::debug!("H3 error: {error}");
    if let H3RequestError::Protocol(code) = error {
        connection.close(code.into(), code.reason().as_bytes());
    }
    h3.shut_down().await;
}
