//! HTTP/3 specific exports

pub mod web_transport;
use crate::{
    ArcHandler, QuicConnection, QuicConnectionTrait, QuicEndpoint, QuicTransportReceive,
    QuicTransportSend, RuntimeTrait,
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
    runtime: impl RuntimeTrait,
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
    runtime: impl RuntimeTrait,
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
    handle_inbound_bidi_streams(connection, h3.clone(), handler, runtime, wt_dispatcher).await;
}

async fn handle_inbound_bidi_streams<QC: QuicConnectionTrait>(
    connection: QC,
    h3: Arc<H3Connection>,
    handler: ArcHandler<impl Handler>,
    runtime: impl RuntimeTrait,
    wt_dispatcher: Option<WebTransportDispatcher>,
) {
    loop {
        match h3.swansong().interrupt(connection.accept_bidi()).await {
            None => {
                log::trace!("H3 bidi accept loop: interrupted by swansong shutdown");
                break;
            }
            Some(Err(e)) => {
                log::debug!("H3 bidi accept loop: accept_bidi error: {e}");
                break;
            }
            Some(Ok((stream_id, transport))) => {
                handle_bidi_stream(
                    stream_id,
                    transport,
                    &h3,
                    &handler,
                    &connection,
                    &runtime,
                    &wt_dispatcher,
                );
            }
        }
    }

    h3.shut_down();
}

fn handle_bidi_stream<QC: QuicConnectionTrait>(
    stream_id: u64,
    transport: QC::BidiStream,
    h3: &Arc<H3Connection>,
    handler: &ArcHandler<impl Handler>,
    connection: &QC,
    runtime: &impl RuntimeTrait,
    wt_dispatcher: &Option<WebTransportDispatcher>,
) {
    log::trace!("H3 bidi stream {stream_id}: spawning handler task");
    let (h3, handler, connection, wt_dispatcher) = (
        h3.clone(),
        handler.clone(),
        connection.clone(),
        wt_dispatcher.clone(),
    );

    runtime.spawn(async move {
        let handler = &handler;
        let peer_ip = connection.remote_address().ip();
        let quic_connection = connection.clone();
        let wt_dispatcher = wt_dispatcher.clone();

        let handler_fn = {
            let wt_dispatcher = wt_dispatcher.clone();
            |mut conn: trillium_http::Conn<_>| async move {
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
        };

        let result = h3
            .clone()
            .process_inbound_bidi_with_reset(transport, handler_fn, stream_id, |t, code| {
                // RFC 9114 §4.1.2: stream-level protocol errors (notably H3_MESSAGE_ERROR)
                // MUST RST the stream. We stop the recv side and reset the send side with
                // the same code so the peer sees the error on whichever direction it's
                // listening on.
                let raw = u64::from(code);
                t.stop(raw);
                t.reset(raw);
            })
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

            Err(error) => {
                log::debug!("H3 bidi stream {stream_id}: error: {error}");
                handle_h3_error(error, &connection, &h3);
            }
        }
    });
}

fn spawn_inbound_uni_streams<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &impl RuntimeTrait,
    wt_dispatcher: &Option<WebTransportDispatcher>,
) {
    let (connection, h3, runtime, wt_dispatcher) = (
        connection.clone(),
        h3.clone(),
        runtime.clone(),
        wt_dispatcher.clone(),
    );
    runtime.clone().spawn(async move {
        while let Some(Ok((_stream_id, recv))) =
            h3.swansong().interrupt(connection.accept_uni()).await
        {
            let (connection, h3, wt_dispatcher) =
                (connection.clone(), h3.clone(), wt_dispatcher.clone());

            runtime.spawn(async move {
                // RFC 9114 §8.1 / RFC 9204 §6 connection-level errors must close the
                // QUIC connection while the recv stream is still alive — otherwise
                // quinn's RecvStream::drop sends STOP_SENDING, and the peer's malformed
                // RESET_STREAM response can race ahead and override our app error code
                // with FINAL_SIZE_ERROR on the wire. The closure fires inside
                // process_inbound_uni_with_close before stream drops, so the close sets
                // quinn's conn.error first and the drop becomes a no-op.
                let close_connection = {
                    let connection = connection.clone();
                    let h3 = h3.clone();
                    move |code: H3ErrorCode| {
                        connection.close(code.into(), code.reason().as_bytes());
                        h3.shut_down();
                    }
                };
                let result = h3
                    .process_inbound_uni_with_close(recv, close_connection)
                    .await;

                match result {
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
                        // Connection-level protocol errors already fired the close
                        // callback above; this call is a no-op for the close path
                        // (idempotent) and still useful for logging plus I/O errors.
                        handle_h3_error(error, &connection, &h3);
                    }
                }
            });
        }

        h3.shut_down();
    });
}

fn spawn_qpack_decoder_stream<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &impl RuntimeTrait,
) {
    let (connection, h3) = (connection.clone(), h3.clone());

    runtime.spawn(async move {
        log::trace!("H3: opening outbound QPACK decoder stream");
        let stream = match connection.open_uni().await {
            Ok((_stream_id, stream)) => stream,
            Err(err) => {
                log::error!("H3: open_uni for QPACK decoder stream failed: {err:?}");
                h3.shut_down();
                return;
            }
        };

        let result = h3.run_decoder(stream).await;

        if let Err(error) = result {
            handle_h3_error(error, &connection, &h3);
        }

        h3.shut_down();
    });
}

fn spawn_qpack_encoder_stream<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &impl RuntimeTrait,
) {
    let (connection, h3) = (connection.clone(), h3.clone());
    runtime.spawn(async move {
        log::trace!("H3: opening outbound QPACK encoder stream");
        let stream = match connection.open_uni().await {
            Ok((_stream_id, stream)) => stream,
            Err(err) => {
                log::error!("H3: open_uni for QPACK encoder stream failed: {err:?}");
                h3.shut_down();
                return;
            }
        };

        let result = h3.run_encoder(stream).await;

        if let Err(error) = result {
            handle_h3_error(error, &connection, &h3);
        }

        h3.shut_down();
    });
}

fn spawn_outbound_control_stream<QC: QuicConnectionTrait>(
    connection: &QC,
    h3: &Arc<H3Connection>,
    runtime: &impl RuntimeTrait,
) {
    let (connection, h3) = (connection.clone(), h3.clone());
    runtime.spawn(async move {
        log::trace!("H3: opening outbound control stream");
        let stream = match connection.open_uni().await {
            Ok((_stream_id, stream)) => stream,
            Err(err) => {
                log::error!("H3: open_uni for outbound control stream failed: {err:?}");
                h3.shut_down();
                return;
            }
        };

        let result = h3.run_outbound_control(stream).await;

        if let Err(error) = result {
            handle_h3_error(error, &connection, &h3);
        }

        h3.shut_down();
    });
}

fn handle_h3_error(error: H3Error, connection: &impl QuicConnectionTrait, h3: &H3Connection) {
    log::debug!("H3 error: {error}");
    if let H3Error::Protocol(code) = error
        && code.is_connection_error()
    {
        connection.close(code.into(), code.reason().as_bytes());
        h3.shut_down();
    }
}
