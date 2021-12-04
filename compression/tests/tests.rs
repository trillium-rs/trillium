use trillium::KnownHeaderName::{AcceptEncoding, ContentEncoding, ContentLength, Vary};
use trillium_testing::prelude::*;

static COMPRESSIBLE_CONTENT: &str = r#"
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated
should be very compressible because it's repeated"#;

#[test]
fn test() {
    let handler = (trillium_compression::compression(), COMPRESSIBLE_CONTENT);

    assert_eq!(COMPRESSIBLE_CONTENT.len(), 500);

    assert_headers!(
        get("/")
            .with_request_header(AcceptEncoding, "br")
            .on(&handler),
        ContentLength => "51",
        Vary => "Accept-Encoding",
        ContentEncoding => "br",
    );

    assert_headers!(
        get("/")
            .with_request_header(AcceptEncoding, "gzip")
            .on(&handler),
        ContentLength => "77",
        Vary => "Accept-Encoding",
        ContentEncoding => "gzip"
    );

    assert_headers!(
        get("/")
            .with_request_header(AcceptEncoding, "deflate")
            .on(&handler),
        ContentLength => "500",
        Vary => None,
        ContentEncoding => None
    );

    assert_headers!(
        get("/")
            .with_request_header(AcceptEncoding, "br;q=0.5, gzip;q=0.75")
            .on(&handler),
        ContentLength => "77",
        ContentEncoding => "gzip"
    );

    assert_headers!(
        get("/")
            .with_request_header(AcceptEncoding, "gzip;q=0.75, br;q=0.5, deflate")
            .on(&handler),
        ContentLength => "77",
        ContentEncoding => "gzip"
    );

    assert_headers!(
        get("/")
            .with_request_header(AcceptEncoding, "deflate, gzip;q=0.75, br;q=0.95")
            .on(&handler),
        ContentLength => "51",
        ContentEncoding => "br"
    );
}
