use afl::fuzz;
use arbitrary::{Arbitrary, Unstructured};
use trillium_fuzzers::async_read::{FuzzTransport, SocketReads};
use trillium_http::{ReceivedBody, ReceivedBodyState};

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
        None,
        transport,
        ReceivedBodyState::Start,
        None,
        encoding_rs::UTF_8,
    );

    let _result = trillium_testing::block_on(body.read_bytes());
    println!("pass");
}

fn main() {
    fuzz!(|data: FuzzInput| { received_body_fuzzer(data) });
}
