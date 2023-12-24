#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SendStatus {
    Success,
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
        self.call(SendStatus::Failure);
    }
}
