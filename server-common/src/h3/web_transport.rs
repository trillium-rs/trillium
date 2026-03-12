//! WebTransport types

use crate::quic::{BoxedBidiStream, BoxedRecvStream};
use std::{
    any::Any,
    fmt::{self, Debug},
    sync::{Arc, RwLock},
};

/// An inbound WebTransport stream, dispatched by [`WebTransportDispatcher`] to the registered
/// handler.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(get)]
pub enum WebTransportStream {
    /// A bidirectional stream.
    Bidi {
        /// The WebTransport session ID (stream ID of the CONNECT request).
        session_id: u64,
        /// The stream transport, ready for application data.
        stream: BoxedBidiStream,
        /// Any bytes buffered after the session ID during stream negotiation.
        buffer: Vec<u8>,
    },
    /// A unidirectional stream.
    Uni {
        /// The WebTransport session ID.
        session_id: u64,
        /// The receive stream, ready for application data.
        stream: BoxedRecvStream,
        /// Any bytes buffered after the session ID during stream negotiation.
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

/// Trait for receiving dispatched WebTransport streams.
///
/// Implementors are registered with [`WebTransportDispatcher`] and receive each inbound stream
/// via [`dispatch`](WebTransportDispatch::dispatch).
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

/// Dispatcher for inbound WebTransport streams on a QUIC connection.
///
/// Bridges the QUIC connection handler, which delivers streams as they arrive, with WebTransport
/// session handlers that register later via [`get_or_init_with`](Self::get_or_init_with).
/// Streams that arrive before a handler registers are buffered and delivered when the handler
/// registers.
///
/// Cheaply cloneable.
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
