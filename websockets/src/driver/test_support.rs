use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

/// One direction of an in-memory connection that holds at most `capacity` bytes. Like a socket, it
/// keeps only the most recent waker for each of reading and writing.
#[derive(Default)]
struct Pipe {
    bytes: VecDeque<u8>,
    capacity: usize,
    closed: bool,
    reader: Option<Waker>,
    writer: Option<Waker>,
}

pub(crate) struct End {
    inbound: Arc<Mutex<Pipe>>,
    outbound: Arc<Mutex<Pipe>>,
}

pub(crate) fn pipe(capacity: usize) -> (End, End) {
    let a = Arc::new(Mutex::new(Pipe {
        capacity,
        ..Pipe::default()
    }));
    let b = Arc::new(Mutex::new(Pipe {
        capacity,
        ..Pipe::default()
    }));
    (
        End {
            inbound: Arc::clone(&a),
            outbound: Arc::clone(&b),
        },
        End {
            inbound: b,
            outbound: a,
        },
    )
}

impl AsyncRead for End {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut pipe = self.inbound.lock().unwrap();
        if pipe.bytes.is_empty() {
            if pipe.closed {
                return Poll::Ready(Ok(0));
            }
            pipe.reader = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let len = buf.len().min(pipe.bytes.len());
        for (slot, byte) in buf.iter_mut().zip(pipe.bytes.drain(..len)) {
            *slot = byte;
        }
        if let Some(writer) = pipe.writer.take() {
            writer.wake();
        }
        Poll::Ready(Ok(len))
    }
}

impl AsyncWrite for End {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut pipe = self.outbound.lock().unwrap();
        let len = buf.len().min(pipe.capacity - pipe.bytes.len());
        if len == 0 && !buf.is_empty() {
            pipe.writer = Some(cx.waker().clone());
            return Poll::Pending;
        }
        pipe.bytes.extend(&buf[..len]);
        if let Some(reader) = pipe.reader.take() {
            reader.wake();
        }
        Poll::Ready(Ok(len))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.close_outbound();
        Poll::Ready(Ok(()))
    }
}

impl End {
    fn close_outbound(&self) {
        let mut pipe = self.outbound.lock().unwrap();
        pipe.closed = true;
        if let Some(reader) = pipe.reader.take() {
            reader.wake();
        }
    }
}

impl Drop for End {
    fn drop(&mut self) {
        self.close_outbound();
    }
}

impl trillium::Transport for End {}

struct Flag(AtomicBool);

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Polls each task whenever its waker has fired, until all of them complete. Returns false if the
/// unfinished tasks are all waiting and nothing will wake them: in these single-threaded tests,
/// that means a wakeup was lost.
pub(crate) fn run_to_completion(tasks: Vec<Pin<Box<dyn Future<Output = ()> + '_>>>) -> bool {
    let mut tasks: Vec<_> = tasks
        .into_iter()
        .map(|task| (Some(task), Arc::new(Flag(AtomicBool::new(true)))))
        .collect();

    loop {
        let mut polled = false;
        for (task, flag) in &mut tasks {
            let Some(future) = task else { continue };
            if !flag.0.swap(false, Ordering::SeqCst) {
                continue;
            }
            polled = true;
            let waker = Waker::from(Arc::clone(flag));
            if future
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_ready()
            {
                *task = None;
            }
        }

        if tasks.iter().all(|(task, _)| task.is_none()) {
            return true;
        }
        if !polled {
            return false;
        }
    }
}
