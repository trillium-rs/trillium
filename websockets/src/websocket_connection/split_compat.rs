// Retained only so that callers of the deprecated `take_inbound_stream` and `inbound_stream` keep
// working; remove it at the next breaking release. Removal is this module, the `mod` line and
// `DriverSlot` field type in `websocket_connection.rs` (the field becomes `Interrupt<Driver>`,
// polled with `driver::poll_inbound`), and the `inbound_taken` branch in `direction.rs`.
//
// An unsplit conn owns its driver outright. Splitting moves the driver behind a mutex shared by the
// conn, which keeps sending, and the taken stream, which reads.

use super::WebSocketConn;
use crate::driver::{Driver, MessageResult, poll_inbound, write_out_unless_message};
use futures_lite::Stream;
use std::{
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    task::{Context, Poll, Wake, Waker},
};
use swansong::Interrupt;
use tungstenite::{Error, Message};

impl WebSocketConn {
    /// Take the inbound message stream from this conn.
    ///
    /// The conn keeps sending, and yields no inbound messages of its own afterwards. Returns
    /// `None` if the stream was already taken.
    #[deprecated(
        note = "a WebSocketConn is itself the stream of inbound messages; poll it directly"
    )]
    pub fn take_inbound_stream(&mut self) -> Option<impl Stream<Item = MessageResult> + use<>> {
        self.driver.split()
    }

    /// Borrow the inbound message stream from this conn, unless it was taken with
    /// [`take_inbound_stream`][Self::take_inbound_stream].
    #[deprecated(
        note = "a WebSocketConn is itself the stream of inbound messages; poll it directly"
    )]
    pub fn inbound_stream(&mut self) -> Option<impl Stream<Item = MessageResult> + '_> {
        if self.driver.is_split() {
            None
        } else {
            Some(self)
        }
    }

    pub(crate) fn inbound_taken(&self) -> bool {
        self.driver.is_split()
    }

    pub(crate) fn poll_write_out(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.driver.poll_write_out(cx)
    }
}

// The large variant is the usual one. Boxing it would add an indirection to every conn to save
// space only on split ones.
#[allow(clippy::large_enum_variant)]
pub(crate) enum DriverSlot {
    Owned(Interrupt<Driver>),
    Shared(Arc<SharedDriver>),
}

impl DriverSlot {
    pub(crate) fn is_split(&self) -> bool {
        matches!(self, Self::Shared(_))
    }

    fn split(&mut self) -> Option<TakenInbound> {
        if self.is_split() {
            return None;
        }

        let shared = Arc::new(SharedDriver {
            driver: Mutex::new(None),
            write_wakers: Arc::default(),
        });

        match std::mem::replace(self, Self::Shared(Arc::clone(&shared))) {
            Self::Owned(driver) => {
                *lock(&shared.driver) = Some(driver);
                Some(TakenInbound(shared))
            }
            already_split => {
                *self = already_split;
                None
            }
        }
    }

    pub(crate) fn poll_ready_to_write(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match self {
            Self::Owned(driver) => driver.poll_ready_to_write(cx),
            Self::Shared(shared) => shared.poll_write(Half::Conn, cx, Driver::poll_ready_to_write),
        }
    }

    pub(crate) fn write(&mut self, message: Message) -> Result<(), Error> {
        match self {
            Self::Owned(driver) => driver.write(message),
            Self::Shared(shared) => shared
                .with_driver(|driver| driver.write(message))
                .unwrap_or(Err(Error::AlreadyClosed)),
        }
    }

    pub(crate) fn close(&mut self) -> Result<(), Error> {
        match self {
            Self::Owned(driver) => driver.close(),
            Self::Shared(shared) => shared
                .with_driver(|driver| driver.close())
                .unwrap_or(Err(Error::AlreadyClosed)),
        }
    }

    pub(crate) fn poll_write_out(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match self {
            Self::Owned(driver) => driver.poll_write_out(cx),
            Self::Shared(shared) => shared.poll_write(Half::Conn, cx, Driver::poll_write_out),
        }
    }

    pub(crate) fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        match self {
            Self::Owned(driver) => driver.poll_flush(cx),
            Self::Shared(shared) => shared.poll_write(Half::Conn, cx, Driver::poll_flush),
        }
    }

    pub(crate) fn poll_inbound(&mut self, cx: &mut Context<'_>) -> Poll<Option<MessageResult>> {
        match self {
            Self::Owned(driver) => poll_inbound(driver, cx),
            Self::Shared(_) => Poll::Ready(None),
        }
    }
}

pub(crate) struct SharedDriver {
    // `None` only while `DriverSlot::split` moves the driver in.
    driver: Mutex<Option<Interrupt<Driver>>>,
    write_wakers: Arc<WriteWakers>,
}

impl SharedDriver {
    fn with_driver<T>(&self, f: impl FnOnce(&mut Interrupt<Driver>) -> T) -> Option<T> {
        lock(&self.driver).as_mut().map(f)
    }

    /// Either half can be left waiting on the socket to write, possibly from separate tasks, and a
    /// socket keeps only the most recent waker it was polled with. Writing with a waker that wakes
    /// both halves keeps one half's wait from replacing the other's.
    fn poll_write(
        &self,
        half: Half,
        cx: &mut Context<'_>,
        poll: impl FnOnce(&mut Driver, &mut Context<'_>) -> Poll<Result<(), Error>>,
    ) -> Poll<Result<(), Error>> {
        self.write_wakers.register(half, cx.waker());
        let waker = Waker::from(Arc::clone(&self.write_wakers));
        let mut cx = Context::from_waker(&waker);
        self.with_driver(|driver| poll(driver, &mut cx))
            .unwrap_or(Poll::Ready(Err(Error::AlreadyClosed)))
    }
}

struct TakenInbound(Arc<SharedDriver>);

impl Stream for TakenInbound {
    type Item = MessageResult;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let shared = &self.0;
        let poll = shared
            .with_driver(|driver| Pin::new(driver).poll_next(cx))
            .unwrap_or(Poll::Ready(None));
        write_out_unless_message(poll, || {
            shared.poll_write(Half::Inbound, cx, Driver::poll_write_out)
        })
    }
}

#[derive(Clone, Copy)]
enum Half {
    Inbound,
    Conn,
}

#[derive(Default)]
struct WriteWakers {
    inbound: Mutex<Option<Waker>>,
    conn: Mutex<Option<Waker>>,
}

impl WriteWakers {
    fn register(&self, half: Half, waker: &Waker) {
        let mut slot = lock(match half {
            Half::Inbound => &self.inbound,
            Half::Conn => &self.conn,
        });
        if !slot
            .as_ref()
            .is_some_and(|registered| registered.will_wake(waker))
        {
            *slot = Some(waker.clone());
        }
    }
}

impl Wake for WriteWakers {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        for slot in [&self.inbound, &self.conn] {
            if let Some(waker) = lock(slot).take() {
                waker.wake();
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{
        default_config,
        test_support::{pipe, run_to_completion},
    };
    use futures_lite::{StreamExt, future::poll_fn};
    use swansong::Swansong;
    use tungstenite::protocol::Role;

    /// While the conn waits to write, the taken stream answers pings, so both halves wait on the
    /// same full socket. Both must be woken when it drains.
    #[test]
    fn both_halves_are_woken_to_write() {
        const COUNT: usize = 32;
        const LEN: usize = 1000;

        let swansong = Swansong::new();
        let (server, client) = pipe(512);
        let mut conn = DriverSlot::Owned(swansong.interrupt(Driver::new(
            Box::new(server),
            Vec::new(),
            Role::Server,
            default_config().write_buffer_size(2048),
        )));
        let mut inbound = conn.split().unwrap();
        let mut peer = swansong.interrupt(Driver::new(
            Box::new(client),
            Vec::new(),
            Role::Client,
            default_config(),
        ));

        let sending = async {
            for i in 0..COUNT {
                poll_fn(|cx| conn.poll_ready_to_write(cx)).await.unwrap();
                conn.write(Message::binary(vec![i as u8; LEN])).unwrap();
            }
            poll_fn(|cx| conn.poll_flush(cx)).await.unwrap();
        };

        let reading = async {
            while let Some(message) = inbound.next().await {
                if message.unwrap() == Message::text("done") {
                    break;
                }
            }
        };

        let peer = async {
            for _ in 0..8 {
                peer.write(Message::Ping("ping".into())).unwrap();
            }
            poll_fn(|cx| peer.poll_flush(cx)).await.unwrap();

            let mut received = 0;
            while received < COUNT {
                let message = poll_fn(|cx| poll_inbound(&mut peer, cx)).await;
                if message.unwrap().unwrap().is_binary() {
                    received += 1;
                }
            }

            peer.write(Message::text("done")).unwrap();
            poll_fn(|cx| peer.poll_flush(cx)).await.unwrap();
        };

        assert!(
            run_to_completion(vec![Box::pin(sending), Box::pin(reading), Box::pin(peer)]),
            "stalled"
        );
    }
}
