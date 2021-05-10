use futures_lite::{io::AsyncRead, ready};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// An encoder for chunked encoding.
#[derive(Debug)]
pub struct ChunkedEncoder<R> {
    reader: R,
    done: bool,
}

impl<R: Unpin + AsyncRead> ChunkedEncoder<R> {
    /// Create a new instance.
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            done: false,
        }
    }
}

impl<R: Unpin + AsyncRead> AsyncRead for ChunkedEncoder<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.done {
            return Poll::Ready(Ok(0));
        }
        let reader = &mut self.reader;

        let max_bytes_to_read = max_bytes_to_read(buf.len());

        let bytes = ready!(Pin::new(reader).poll_read(cx, &mut buf[..max_bytes_to_read]))?;
        if bytes == 0 {
            self.done = true;
        }
        let start = format!("{:X}\r\n", bytes);
        let start_length = start.as_bytes().len();
        let total = bytes + start_length + 2;
        buf.copy_within(..bytes, start_length);
        buf[..start_length].copy_from_slice(start.as_bytes());
        buf[total - 2..total].copy_from_slice(b"\r\n");
        Poll::Ready(Ok(total))
    }
}

fn max_bytes_to_read(buf_len: usize) -> usize {
    if buf_len < 6 {
        // the minimum read size is of 6 represents one byte of
        // content from the body. the other five bytes are 1\r\n_\r\n
        // where _ is the actual content in question
        panic!("buffers of length {} are too small for this implementation. if this is a problem for you, please open an issue", buf_len);
    }

    let bytes_remaining_after_two_cr_lns = (buf_len - 4) as f64;

    // the maximum number of bytes that the hex representation of remaining bytes might take
    let max_bytes_of_hex_framing = bytes_remaining_after_two_cr_lns.log2() / 4f64;

    (bytes_remaining_after_two_cr_lns - max_bytes_of_hex_framing.ceil()) as usize
}

#[cfg(test)]
mod test_bytes_to_read {
    #[test]
    fn simple_check_of_known_values() {
        // the marked rows are the most important part of this test,
        // and a nonobvious but intentional consequence of the
        // implementation. in order to avoid overflowing, we must use
        // one fewer than the available buffer bytes because
        // increasing the read size increase the number of framed
        // bytes by two. This occurs when the hex representation of
        // the content bytes is near an increase in order of magnitude
        // (F->10, FF->100, FFF-> 1000, etc)
        let values = vec![
            (6, 1),       // 1
            (7, 2),       // 2
            (20, 15),     // F
            (21, 15),     // F <-
            (22, 16),     // 10
            (23, 17),     // 11
            (260, 254),   // FE
            (261, 254),   // FE <-
            (262, 255),   // FF <-
            (263, 256),   // 100
            (4100, 4093), // FFD
            (4101, 4093), // FFD <-
            (4102, 4094), // FFE <-
            (4103, 4095), // FFF <-
            (4104, 4096), // 1000
        ];

        for (input, expected) in values {
            let actual = super::max_bytes_to_read(input);
            assert_eq!(
                actual, expected,
                "\n\nexpected max_bytes_to_read({}) to be {}, but it was {}",
                input, expected, actual
            );

            // testing the test:
            let used_bytes = expected + 4 + format!("{:X}", expected).len();
            assert!(
                used_bytes == input || used_bytes == input - 1,
                "\n\nfor an input of {}, expected used bytes to be {} or {}, but was {}",
                input,
                input,
                input - 1,
                used_bytes
            );
        }
    }
}
