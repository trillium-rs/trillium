//! HTTP/3 specific exports

pub mod web_transport;
use crate::{
    ArcHandler, QuicConnection, QuicConnectionTrait, QuicEndpoint, QuicTransportReceive,
    QuicTransportSend, Runtime,
};
use std::sync::Arc;
use trillium::{Handler, Upgrade};
use trillium_http::{
    HttpContext,
    h3::{H3Connection, H3Error, H3ErrorCode, H3StreamResult, UniStreamResult},
};
use web_transport::{WebTransportDispatcher, WebTransportStream};

/// A QUIC stream identifier
#[derive(Clone, Copy, Debug)]
pub struct StreamId(u64);
impl From<StreamId> for u64 {
    fn from(val: StreamId) -> Self {
        val.0
    }
}

impl From<u64> for StreamId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

pub(crate) async fn run_h3<QE: QuicEndpoint>(
    quic_binding: QE,
    context: Arc<HttpContext>,
    handler: ArcHandler<impl Handler>,
    runtime: Runtime,
) {
    let swansong = context.swansong();
    while let Some(connection) = swansong.interrupt(quic_binding.accept()).await.flatten() {
        let h3 = H3Connection::new(context.clone());
        let handler = handler.clone();
        let runtime = runtime.clone();
        runtime
            .clone()
            .spawn(run_h3_connection(connection, h3, handler, runtime));
    }
}

async fn run_h3_connection<QC: QuicConnectionTrait>(
    connection: QC,
    h3: Arc<H3Connection>,
    handler: ArcHandler<impl Handler>,
    runtime: Runtime,
) {
    let wt_dispatcher = h3
        .context()
        .config()
        .webtransport_enabled()
        .then(WebTransportDispatcher::new);

    log::trace!("new quic connection from {}", connection.remote_address());

    spawn_outbound_control_stream(&connection, &h3, &runtime);
    spawn_qpack_encoder_stream(&connection, &h3, &runtime);
    spawn_qpack_decoder_stream(&connection, &h3, &runtime);
    spawn_inbound_uni_streams(&connection, &h3, &runtime, &wt_dispatcher);
    handle_inbound_bidi_streams(connection, h3, handler, runtime, wt_dispatcher).await;
}

async fn handle_inbound_bidi_streams<QC: QuicConnectionTrait>(
    connection: QC,
    h3: Arc<H3Connection>,
    handler: ArcHandler<impl Handler>,
    runtime: Runtime,
    wt_dispatcher: Option<WebTransportDispatcher>,
) {
    let swansong = h3.swansong().clone();
    while let Some(Ok((stream_id, transport))) = swansong.interrupt(connection.accept_bidi()).await
    {
        let (h3, handler, connection, wt_dispatcher) = (
            h3.clone(),
            handler.clone(),
            connection.clone(),
            wt_dispatcher.clone(),
        );
        let peer_ip = connection.remote_address().ip();
        runtime.spawn(async move {
            let handler = &handler;
            let quic_connection = connection.clone();
            let wt_dispatcher = wt_dispatcher.clone();
            let result = h3
                .clone()
                .process_inbound_bidi(
                    transport,
                    {
                        let wt_dispatcher = wt_dispatcher.clone();
                        |mut conn| async move {
                            conn.set_peer_ip(Some(peer_ip));
                            conn.set_secure(true);
                            let state = conn.state_mut();
                            state.insert(quic_connection.clone());
                            state.insert(QuicConnection::from(quic_connection));
                            state.insert(StreamId(stream_id));
                            if let Some(dispatcher) = wt_dispatcher {
                                state.insert(dispatcher);
                            }
                            let conn = handler.run(conn.into()).await;
                            let conn = handler.before_send(conn).await;
                            conn.into_inner()
                        }
                    },
                    stream_id,
                )
                .await;

            match result {
                Ok(H3StreamResult::Request(conn)) if conn.should_upgrade() => {
                    let upgrade = Upgrade::from(conn);
                    if handler.has_upgrade(&upgrade) {
                        log::debug!("upgrading h3 stream");
                        handler.upgrade(upgrade).await;
                    } else {
                        log::error!("h3 upgrade specified but no upgrade handler provided");
                    }
                }
                Ok(H3StreamResult::Request(_)) => {}
                Ok(H3StreamResult::WebTransport {
                    session_id,
                    mut transport,
                    buffer,
                }) => {
                    if let Some(dispatcher) = &wt_dispatcher {
                        dispatcher.dispatch(WebTransportStream::Bidi {
                            session_id,
                            stream: Box::new(transport),
                            buffer: buffer.into(),
                        });
                    } else {
                        transport.stop(H3ErrorCode::StreamCreationError.into());
                        transport.reset(H3ErrorCode::StreamCreationError.into());
                    }
                }
                Err(error) => handle_h3_error(error, &connection, &h3).await,
            }
        });
    }
}

fn spawn_inbound_uni_streams<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &Runtime,
    wt_dispatcher: &Option<WebTransportDispatcher>,
) {
    let (connection, h3, runtime, wt_dispatcher) = (
        connection.clone(),
        h3.clone(),
        runtime.clone(),
        wt_dispatcher.clone(),
    );
    runtime.clone().spawn(async move {
        while let Ok((_stream_id, recv)) = connection.accept_uni().await {
            let (connection, h3, wt_dispatcher) =
                (connection.clone(), h3.clone(), wt_dispatcher.clone());
            runtime.spawn(async move {
                match h3.process_inbound_uni(recv).await {
                    Ok(UniStreamResult::Handled) => {}
                    Ok(UniStreamResult::WebTransport {
                        session_id,
                        mut stream,
                        buffer,
                    }) => {
                        if let Some(dispatcher) = &wt_dispatcher {
                            dispatcher.dispatch(WebTransportStream::Uni {
                                session_id,
                                stream: Box::new(stream),
                                buffer: buffer.into(),
                            });
                        } else {
                            stream.stop(H3ErrorCode::StreamCreationError.into());
                        }
                    }
                    Ok(UniStreamResult::Unknown { mut stream, .. }) => {
                        stream.stop(H3ErrorCode::StreamCreationError.into());
                    }
                    Err(error) => {
                        handle_h3_error(error, &connection, &h3).await;
                    }
                }
            });
        }
    });
}

fn spawn_qpack_decoder_stream<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &Runtime,
) {
    let (connection, h3) = (connection.clone(), h3.clone());
    runtime.spawn(async move {
        let result: Result<(), H3Error> =
            async { h3.run_decoder(connection.open_uni().await?.1).await }.await;
        if let Err(error) = result {
            handle_h3_error(error, &connection, &h3).await;
        }
    });
}

fn spawn_qpack_encoder_stream<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &Runtime,
) {
    let (connection, h3) = (connection.clone(), h3.clone());
    runtime.spawn(async move {
        let result: Result<(), H3Error> =
            async { h3.run_encoder(connection.open_uni().await?.1).await }.await;
        if let Err(error) = result {
            handle_h3_error(error, &connection, &h3).await;
        }
    });
}

fn spawn_outbound_control_stream<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &Runtime,
) {
    let (connection, h3) = (connection.clone(), h3.clone());
    runtime.spawn(async move {
        let guard = h3.swansong().guard();

        let result: Result<(), H3Error> = async {
            h3.run_outbound_control(connection.open_uni().await?.1)
                .await
        }
        .await;
        drop(guard);
        if let Err(error) = result {
            handle_h3_error(error, &connection, &h3).await;
        }
    });
}

async fn handle_h3_error(error: H3Error, connection: &impl QuicConnectionTrait, h3: &H3Connection) {
    log::debug!("H3 error: {error}");
    if let H3Error::Protocol(code) = error {
        if code.is_connection_error() {
            // Connection-level protocol error: close the QUIC connection and signal all
            // in-progress tasks to stop.
            connection.close(code.into(), code.reason().as_bytes());
            h3.shut_down().await;
        }
        // Stream-level protocol errors (MessageError, RequestIncomplete, StreamCreationError,
        // NoError, etc.) affect only the individual stream; the connection stays open.
    }
    // I/O errors (e.g. stream reset by peer) are stream-level; do not shut down the
    // whole connection. The connection lifecycle cleans itself up when accept_bidi() fails.
}
