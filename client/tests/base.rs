use std::str::FromStr;
use test_harness::test;
use trillium_client::{Client, Status};
use trillium_testing::{harness, ServerConnector, TestResult};
use url::Url;

fn test_client() -> Client {
    Client::new(ServerConnector::new(Status::Ok))
}

#[test(harness)]
async fn with_base() -> TestResult {
    let client = test_client().with_base("http://example.com/a/b");

    assert_eq!(client.get("c").url().as_str(), "http://example.com/a/b/c");

    assert_eq!(client.build_url("/c")?.as_str(), "http://example.com/a/b/c");

    assert_eq!(
        client
            .build_url(Url::from_str("http://example.com/a/b/c/d")?)?
            .as_str(),
        "http://example.com/a/b/c/d"
    );

    assert!(client
        .build_url(Url::from_str("http://example.com/a")?) // does not start with http://example.com/a/b/
        .is_err());

    assert!(client
        .build_url("http://example.test/") // does not start with http://example.com/a/b/
        .is_err());

    Ok(())
}

#[test(harness)]
async fn without_base() -> TestResult {
    let client = test_client();

    assert_eq!(
        client.build_url("http://example.com/a/b/c")?.as_str(),
        "http://example.com/a/b/c"
    );

    assert_eq!(
        client
            .build_url(Url::from_str("http://example.com/a/b/c")?)?
            .as_str(),
        "http://example.com/a/b/c"
    );

    assert!(client.build_url("/a/b/c").is_err());

    assert!(client.build_url("data:text/plain,Stuff").is_err());
    assert!(client
        .build_url(Url::from_str("data:text/plain,Stuff")?)
        .is_err());

    Ok(())
}

#[test(harness)]
async fn base_without_trailing_slash() -> TestResult {
    let client = test_client().with_base("http://example.com/a/b");
    // the default behavior is "http://example.com/a/c"
    assert_eq!(client.build_url("c")?.as_str(), "http://example.com/a/b/c");
    Ok(())
}

#[test(harness)]
async fn url_with_leading_slash() -> TestResult {
    let client = test_client().with_base("http://example.com/a/b");
    // the default behavior is "http://example.com/c"
    assert_eq!(client.build_url("/c")?.as_str(), "http://example.com/a/b/c");
    Ok(())
}
