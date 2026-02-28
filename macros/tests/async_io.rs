use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, future::block_on};
use trillium_macros::{AsyncRead, AsyncWrite};
#[derive(AsyncRead, AsyncWrite)]
struct Inner(#[async_write] Vec<u8>, #[async_read] &'static [u8]);

#[derive(AsyncRead, AsyncWrite)]
struct Middle((), #[async_io] Inner);

#[derive(AsyncRead, AsyncWrite)]
struct Outer(Middle);

#[test]
fn test() -> std::io::Result<()> {
    let mut outer = Outer(Middle((), Inner(vec![100; 0], b"content to read")));
    let mut string = String::new();
    block_on(outer.read_to_string(&mut string))?;
    assert_eq!(string, "content to read");
    block_on(outer.write_all(b"written content"))?;
    assert_eq!(outer.0.1.0, b"written content");
    Ok(())
}

#[derive(AsyncRead, AsyncWrite)]
struct InnerNamed {
    #[async_write]
    write: Vec<u8>,
    #[async_read]
    read: &'static [u8],
}

#[derive(AsyncRead, AsyncWrite)]
struct MiddleNamed {
    #[allow(unused)]
    irrelevant: &'static str,
    #[async_io]
    inner: InnerNamed,
}

#[derive(AsyncRead, AsyncWrite)]
struct OuterNamed {
    middle: MiddleNamed,
}

#[test]
fn test_named() -> std::io::Result<()> {
    let mut outer = OuterNamed {
        middle: MiddleNamed {
            irrelevant: "unrelated",
            inner: InnerNamed {
                write: vec![100; 0],
                read: b"content to read",
            },
        },
    };
    let mut string = String::new();
    block_on(outer.read_to_string(&mut string))?;
    assert_eq!(string, "content to read");
    block_on(outer.write_all(b"written content"))?;
    assert_eq!(outer.middle.inner.write, b"written content");
    Ok(())
}

#[test]
fn test_generic() -> std::io::Result<()> {
    #[derive(AsyncRead, AsyncWrite)]
    struct Generic<T>(T);

    let mut writable = Generic(vec![100; 0]);
    block_on(writable.write_all(b"content to write"))?;
    assert_eq!(writable.0, b"content to write");

    let mut readable = Generic(b"test".as_slice());
    let mut string = String::new();
    block_on(readable.read_to_string(&mut string))?;
    assert_eq!("test", string);

    Ok(())
}
