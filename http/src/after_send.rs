/// Outcome reported to [`Conn::after_send`][crate::Conn::after_send] callbacks.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum SendStatus {
    /// The response was fully transferred to the peer.
    Success,
    /// The response did not complete — the conn was dropped before send finished,
    /// the handler panicked, or the transport reported an error mid-write.
    Failure,
}
impl From<bool> for SendStatus {
    fn from(success: bool) -> Self {
        if success {
            Self::Success
        } else {
            Self::Failure
        }
    }
}

impl SendStatus {
    pub fn is_success(self) -> bool {
        SendStatus::Success == self
    }
}

/// Storage for callbacks registered via [`Conn::after_send`][crate::Conn::after_send].
///
/// Callbacks are guaranteed to fire **exactly once** for the lifetime of a `Conn`:
/// either the codec's send path invokes [`call`](Self::call) with the real outcome,
/// or — if the `Conn` is dropped before send completes — the [`Drop`] impl fires
/// the callback with [`SendStatus::Failure`]. Multiple registrations chain in
/// registration order via [`append`](Self::append).
#[derive(Default)]
pub(crate) struct AfterSend(Option<Box<dyn FnOnce(SendStatus) + Send + Sync + 'static>>);

impl AfterSend {
    pub(crate) fn call(&mut self, send_status: SendStatus) {
        if let Some(after_send) = self.0.take() {
            after_send(send_status);
        }
    }

    pub(crate) fn append<F>(&mut self, after_send: F)
    where
        F: FnOnce(SendStatus) + Send + Sync + 'static,
    {
        self.0 = Some(match self.0.take() {
            Some(existing_after_send) => Box::new(move |ss| {
                existing_after_send(ss);
                after_send(ss);
            }),
            None => Box::new(after_send),
        });
    }
}

impl Drop for AfterSend {
    fn drop(&mut self) {
        // Fallback so the callback fires exactly once even when the conn drops
        // before the codec's send path runs (handler panic, transport error,
        // mid-write disconnect, prematurely-released conn). The h1/h2/h3 send
        // paths normally `call` with the real status; Drop covers everything else.
        self.call(SendStatus::Failure);
    }
}
