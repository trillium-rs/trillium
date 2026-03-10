//! WebTransport types

use std::{
    any::Any,
    fmt::{self, Debug},
    sync::{Arc, RwLock},
};
use trillium_http::transport::BoxedTransport;

type BoxedRecvStream = Box<dyn futures_lite::AsyncRead + Unpin + Send + Sync>;

/// An inbound WebTransport stream, dispatched by [`WebTransportDispatcher`] to the registered
/// handler.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(get)]
pub enum WebTransportStream {
    /// A bidirectional stream (signal value 0x41).
    Bidi {
        /// The WebTransport session ID (stream ID of the CONNECT request).
        #[field(copy)]
        session_id: u64,
        /// The transport, with signal value and session ID already consumed.
        stream: BoxedTransport,
        /// Any bytes buffered past the session ID during parsing.
        buffer: Vec<u8>,
    },
    /// A unidirectional stream (stream type 0x54).
    Uni {
        /// The WebTransport session ID.
        session_id: u64,
        /// The receive stream, with stream type and session ID already consumed.
        stream: BoxedRecvStream,
        /// Any bytes buffered past the session ID during parsing.
        buffer: Vec<u8>,
    },
}

impl Debug for WebTransportStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bidi { session_id, .. } => f
                .debug_struct("WebTransportStream::Bidi")
                .field("session_id", session_id)
                .finish_non_exhaustive(),
            Self::Uni { session_id, .. } => f
                .debug_struct("WebTransportStream::Uni")
                .field("session_id", session_id)
                .finish_non_exhaustive(),
        }
    }
}

/// Trait for handling dispatched WebTransport streams.
///
/// Implementors receive inbound streams via [`dispatch`](WebTransportDispatch::dispatch) and
/// manage per-session routing. The `Any` supertrait enables the dispatcher to return a
/// type-erased handler that callers can downcast to their concrete type.
pub trait WebTransportDispatch: Any + Send + Sync {
    /// Handle an inbound WebTransport stream.
    fn dispatch(&self, stream: WebTransportStream);
}

/// Routing state for inbound WebTransport streams on a single QUIC connection.
enum DispatchState {
    /// No handler registered yet. Early-arriving streams are buffered.
    Buffering(Vec<WebTransportStream>),

    /// A handler has been registered. Streams are dispatched directly.
    Active(Arc<dyn WebTransportDispatch>),
}

/// Per-QUIC-connection dispatcher for inbound WebTransport streams.
///
/// Created by the H3 connection handler when WebTransport is enabled in the server config.
/// Inserted into each `Conn`'s state so that the WebTransport handler can retrieve it during
/// upgrade and register itself as the stream consumer.
///
/// Cheaply cloneable (wraps an `Arc`).
#[derive(Clone)]
pub struct WebTransportDispatcher(Arc<RwLock<DispatchState>>);

impl Debug for WebTransportDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.0.read().expect("dispatcher lock poisoned");
        let label = match &*state {
            DispatchState::Buffering(buf) => {
                format!("Buffering({} streams)", buf.len())
            }
            DispatchState::Active(_) => "Active".to_string(),
        };
        f.debug_tuple("WebTransportDispatcher")
            .field(&label)
            .finish()
    }
}

impl WebTransportDispatcher {
    /// Create a new dispatcher in the buffering state.
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(DispatchState::Buffering(Vec::new()))))
    }

    /// Dispatch an inbound WebTransport stream to the registered handler, or buffer it.
    pub fn dispatch(&self, stream: WebTransportStream) {
        // Fast path: handler is registered, take a read lock.
        {
            let state = self.0.read().expect("dispatcher lock poisoned");
            if let DispatchState::Active(handler) = &*state {
                handler.dispatch(stream);
                return;
            }
        }

        // Slow path: still buffering, take a write lock.
        {
            let mut state = self.0.write().expect("dispatcher lock poisoned");
            match &*state {
                DispatchState::Buffering(_) => {
                    let DispatchState::Buffering(buf) = &mut *state else {
                        unreachable!()
                    };
                    buf.push(stream);
                }
                DispatchState::Active(handler) => handler.dispatch(stream),
            }
        }
    }

    /// Get or initialize the dispatch handler.
    ///
    /// If no handler is registered yet, calls `init` to create one, transitions from
    /// buffering to active, and drains any buffered streams through the new handler.
    ///
    /// If a handler is already registered and its concrete type matches `T`, returns
    /// a clone of the existing `Arc<T>`.
    ///
    /// Returns `None` if a handler is already registered but is a different concrete type.
    pub fn get_or_init_with<T: WebTransportDispatch>(
        &self,
        init: impl FnOnce() -> T,
    ) -> Option<Arc<T>> {
        // Fast path: already active.
        {
            let state = self.0.read().expect("dispatcher lock poisoned");
            if let DispatchState::Active(handler) = &*state {
                return downcast_arc(handler.clone());
            }
        }

        // Slow path: take write lock, initialize if still buffering.
        let mut state = self.0.write().expect("dispatcher lock poisoned");
        match &*state {
            DispatchState::Active(handler) => downcast_arc(handler.clone()),
            DispatchState::Buffering(_) => {
                let handler = Arc::new(init());
                let buffered = std::mem::replace(
                    &mut *state,
                    DispatchState::Active(handler.clone() as Arc<dyn WebTransportDispatch>),
                );
                let DispatchState::Buffering(buffered) = buffered else {
                    unreachable!()
                };
                drop(state);

                for stream in buffered {
                    handler.dispatch(stream);
                }

                Some(handler)
            }
        }
    }
}

fn downcast_arc<T: Any + Send + Sync>(arc: Arc<dyn WebTransportDispatch>) -> Option<Arc<T>> {
    let any: Arc<dyn Any + Send + Sync> = arc;
    any.downcast::<T>().ok()
}
