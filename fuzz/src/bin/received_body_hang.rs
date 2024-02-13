#![feature(allocator_api, slice_ptr_get)]

//! This fuzzer is tailored to find possible causes of an issue seen in the wild where a task was
//! woken up in a tight loop, and profiling indicated it was busy reading an HTTP response body.

use std::{future::Future, pin::pin, process::exit, task::Context};

use afl::fuzz;
use arbitrary::{Arbitrary, Unstructured};
use trillium_fuzzers::{
    async_read::{FuzzTransport, SocketReads},
    waker::stub_waker,
};
use trillium_http::{ReceivedBody, ReceivedBodyState};

const MAX_POLLS: usize = 1000;

/// A substitute `GlobalAlloc` implementation that wraps the `System` allocator, and terminates the
/// program with a success status code whenever an allocation fails. This is done to have the fuzzer
/// skip what would otherwise be OOM crashes, which are not presently of interest.
mod suppress_allocator {
    use std::{
        alloc::{Allocator, GlobalAlloc, System},
        process::exit,
        ptr::NonNull,
    };

    #[global_allocator]
    static ALLOCATOR: SuppressErrorAllocator = SuppressErrorAllocator;

    struct SuppressErrorAllocator;

    unsafe impl GlobalAlloc for SuppressErrorAllocator {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            match System.allocate(layout) {
                Ok(ptr) => ptr.as_non_null_ptr().as_ptr(),
                Err(e) => {
                    eprintln!("{e}");
                    // return success so we can ignore all crashes related to memory allocation.
                    exit(0);
                }
            }
        }

        unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
            match System.allocate_zeroed(layout) {
                Ok(ptr) => ptr.as_non_null_ptr().as_ptr(),
                Err(e) => {
                    eprintln!("{e}");
                    // return success so we can ignore all crashes related to memory allocation.
                    exit(0);
                }
            }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            if let Some(ptr) = NonNull::new(ptr) {
                System.deallocate(ptr, layout)
            }
        }
    }
}

#[derive(Debug)]
struct FuzzInput {
    /// This sets the `content_length` field in the `ReceivedBody`. The field is documented as
    /// follows: "Returns the content-length of this body, if available. This usually is derived
    /// from the content-length header. If the http request or response that this body is attached
    /// to uses transfer-encoding chunked, this will be None."
    content_length: Option<u64>,
    /// Provides the data that will be supplied through the facade transport.
    socket_reads: SocketReads,
}

impl<'a> Arbitrary<'a> for FuzzInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let content_length = Arbitrary::arbitrary(u)?;
        let socket_reads = Arbitrary::arbitrary(u)?;
        Ok(Self {
            content_length,
            socket_reads,
        })
    }
}

fn received_body_fuzzer(input: FuzzInput) {
    eprintln!("Input: {input:?}");
    let strings = input.socket_reads.from_utf8_lossy();
    eprintln!("{strings:?}");

    let transport = FuzzTransport::new(input.socket_reads.clone());
    let body = ReceivedBody::new(
        // None means "Transfer-Encoding: chunked", Some(_) means "Content-Length: <length>".
        input.content_length,
        trillium_http::Buffer::default(),
        transport,
        ReceivedBodyState::Start,
        None,
        encoding_rs::UTF_8,
    );
    let (waker, waker_inner) = stub_waker();
    let mut context = Context::from_waker(&waker);
    let mut read_bytes_future = pin!(body.read_bytes());

    for _ in 0..MAX_POLLS {
        if Future::poll(read_bytes_future.as_mut(), &mut context).is_ready() {
            println!("pass");
            return;
        }
    }

    eprintln!("did not finish reading body after {MAX_POLLS} polls");
    eprintln!("woke up {} times", waker_inner.wake_count());
    exit(1);
}

fn main() {
    fuzz!(|data: FuzzInput| { received_body_fuzzer(data) });
}
