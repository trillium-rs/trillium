//! HTTP/3 specific exports

mod priority;
pub mod web_transport;
use crate::{
    ArcHandler, ArcedQuicEndpoint, BoxedBidiStream, QuicConnection, QuicTransportReceive,
    QuicTransportSend, RuntimeTrait, unmap_ipv4,
};
use priority::{PrioritizedStream, PriorityRegistry, transport_priority};
use std::sync::Arc;
use trillium::{Handler, KnownHeaderName, Listener, Upgrade};
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

pub(crate) async fn run_h3(
    quic_binding: ArcedQuicEndpoint,
    context: Arc<HttpContext>,
    handler: ArcHandler<impl Handler>,
    runtime: impl RuntimeTrait,
    listener: Option<Listener>,
    local_alt_svc: Option<&'static str>,
) {
    let swansong = context.swansong();
    while let Some(connection) = swansong.interrupt(quic_binding.accept()).await.flatten() {
        let h3 = H3Connection::new(context.clone());
        let handler = handler.clone();
        let runtime = runtime.clone();
        runtime.clone().spawn(run_h3_connection(
            connection,
            h3,
            handler,
            runtime,
            listener.clone(),
            local_alt_svc,
        ));
    }
}

async fn run_h3_connection(
    connection: QuicConnection,
    h3: Arc<H3Connection>,
    handler: ArcHandler<impl Handler>,
    runtime: impl RuntimeTrait,
    listener: Option<Listener>,
    local_alt_svc: Option<&'static str>,
) {
    let wt_dispatcher = h3
        .context()
        .config()
        .webtransport_enabled()
        .then(WebTransportDispatcher::new);

    log::trace!("new quic connection from {}", connection.remote_address());

    let priorities = PriorityRegistry::default();
    h3.register_priority_callback({
        let priorities = priorities.clone();
        move |stream_id, priority, is_update| {
            priorities.apply(stream_id, transport_priority(priority), is_update)
        }
    });

    spawn_outbound_control_stream(&connection, &h3, &runtime);
    spawn_qpack_encoder_stream(&connection, &h3, &runtime);
    spawn_qpack_decoder_stream(&connection, &h3, &runtime);
    spawn_inbound_uni_streams(&connection, &h3, &runtime, &wt_dispatcher);
    handle_inbound_bidi_streams(
        connection,
        h3.clone(),
        handler,
        runtime,
        wt_dispatcher,
        listener,
        local_alt_svc,
        priorities,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn handle_inbound_bidi_streams(
    connection: QuicConnection,
    h3: Arc<H3Connection>,
    handler: ArcHandler<impl Handler>,
    runtime: impl RuntimeTrait,
    wt_dispatcher: Option<WebTransportDispatcher>,
    listener: Option<Listener>,
    local_alt_svc: Option<&'static str>,
    priorities: PriorityRegistry,
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
                    listener.clone(),
                    local_alt_svc,
                    &priorities,
                );
            }
        }
    }

    h3.shut_down();
}

#[allow(clippy::too_many_arguments)]
fn handle_bidi_stream(
    stream_id: u64,
    transport: BoxedBidiStream,
    h3: &Arc<H3Connection>,
    handler: &ArcHandler<impl Handler>,
    connection: &QuicConnection,
    runtime: &impl RuntimeTrait,
    wt_dispatcher: &Option<WebTransportDispatcher>,
    listener: Option<Listener>,
    local_alt_svc: Option<&'static str>,
    priorities: &PriorityRegistry,
) {
    log::trace!("H3 bidi stream {stream_id}: spawning handler task");
    let (h3, handler, connection, wt_dispatcher, priorities) = (
        h3.clone(),
        handler.clone(),
        connection.clone(),
        wt_dispatcher.clone(),
        priorities.clone(),
    );

    // Wrap the stream so RFC 9218 priority signals routed to its slot are applied to the QUIC
    // send stream as it writes. trillium-http emits the initial priority and any PRIORITY_UPDATE
    // to the connection callback, which stores into this slot.
    let slot = priorities.register(stream_id);
    let transport: BoxedBidiStream = Box::new(PrioritizedStream::new(transport, slot, stream_id));

    runtime.spawn(async move {
        // Unmapped for the same reason the TCP accept loop unmaps: the QUIC listener is bound
        // dual-stack too, so an IPv4 client over HTTP/3 arrives as `::ffff:a.b.c.d`. Without this
        // the same client would key and log differently over h3 than over h1/h2.
        let peer_ip = unmap_ipv4(connection.remote_address().ip());
        let quic_connection = connection.clone();
        let wt_dispatcher = wt_dispatcher.clone();

        let handler_fn = {
            let handler = handler.clone();
            let wt_dispatcher = wt_dispatcher.clone();
            move |mut conn: trillium_http::Conn<_>| async move {
                conn.set_peer_ip(Some(peer_ip));
                conn.set_secure(true);

                let state = conn.state_mut();
                state.insert(quic_connection);
                state.insert(StreamId(stream_id));
                if let Some(listener) = listener {
                    if let Some(addr) = listener.socket_addr() {
                        state.insert(addr);
                    }
                    state.insert(listener);
                }
                if let Some(dispatcher) = wt_dispatcher {
                    state.insert(dispatcher);
                }
                if let Some(alt_svc) = local_alt_svc {
                    conn.response_headers_mut()
                        .try_insert(KnownHeaderName::AltSvc, alt_svc);
                }

                let conn = handler.run(conn.into()).await;
                let conn = handler.before_send(conn).await;

                conn.into_inner()
            }
        };

        let result = h3
            .clone()
            .process_inbound_bidi(transport, handler_fn, stream_id)
            .with_reset(|t, code| {
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

        priorities.deregister(stream_id);
    });
}

fn spawn_inbound_uni_streams(
    connection: &QuicConnection,
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

fn spawn_qpack_decoder_stream(
    connection: &QuicConnection,
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
        // run_decoder shuts the connection down on return.
    });
}

fn spawn_qpack_encoder_stream(
    connection: &QuicConnection,
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
        // run_encoder shuts the connection down on return.
    });
}

fn spawn_outbound_control_stream(
    connection: &QuicConnection,
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
        // run_outbound_control shuts the connection down on return.
    });
}

fn handle_h3_error(error: H3Error, connection: &QuicConnection, h3: &H3Connection) {
    log::debug!("H3 error: {error}");
    if let H3Error::Protocol(code) = error
        && code.is_connection_error()
    {
        connection.close(code.into(), code.reason().as_bytes());
        h3.shut_down();
    }
}
