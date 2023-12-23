use event_listener::Event;

use futures_lite::AsyncRead;

use sluice::pipe::PipeReader;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use trillium::{Conn, KnownHeaderName};

use trillium_http::Body;

use crate::bytes;

struct BodyProxyReader {
    reader: PipeReader,
    started: Option<Arc<(Event, AtomicBool)>>,
}

impl Drop for BodyProxyReader {
    fn drop(&mut self) {
        // if we haven't started yet, notify the copy future that we're not going to
        if let Some(started) = self.started.take() {
            started.0.notify(usize::MAX);
        }
    }
}

impl AsyncRead for BodyProxyReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(started) = self.started.take() {
            started.1.store(true, Ordering::SeqCst);
            started.0.notify(usize::MAX);
        }
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}

pub(crate) fn stream_body(conn: &mut Conn) -> (impl Future<Output = ()> + Send + Sync + '_, Body) {
    let started = Arc::new((Event::new(), AtomicBool::from(false)));
    let started_clone = started.clone();
    let (reader, writer) = sluice::pipe::pipe();
    let len = conn
        .request_headers()
        .get_str(KnownHeaderName::ContentLength)
        .and_then(|s| s.parse().ok());

    (
        async move {
            log::trace!("waiting to stream request body");
            started_clone.0.listen().await;
            if started_clone.1.load(Ordering::SeqCst) {
                log::trace!("started to stream request body");
                let received_body = conn.request_body().await;
                match trillium_http::copy(received_body, writer, 4).await {
                    Ok(streamed) => {
                        log::trace!("streamed {} request body bytes", bytes(streamed))
                    }
                    Err(e) => log::error!("request body stream error: {e}"),
                };
            } else {
                log::trace!("not streaming request body");
            }
        },
        Body::new_streaming(
            BodyProxyReader {
                started: Some(started),
                reader,
            },
            len,
        ),
    )
}
