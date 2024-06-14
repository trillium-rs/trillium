use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
};
use trillium_client::{IntoUrl, Url};

#[test]
fn socket_addr() {
    assert_eq!(
        SocketAddr::from(([127, 0, 0, 1], 8080))
            .into_url(None)
            .unwrap()
            .as_str(),
        "http://127.0.0.1:8080/"
    );

    assert_eq!(
        SocketAddr::from((IpAddr::from_str("::").unwrap(), 8080))
            .into_url(None)
            .unwrap()
            .as_str(),
        "http://[::]:8080/"
    );

    assert_eq!(
        SocketAddr::from_str("[2610:28:3090:3000:0:bad:cafe:47]:443")
            .unwrap()
            .into_url(None)
            .unwrap()
            .as_str(),
        "https://[2610:28:3090:3000:0:bad:cafe:47]/"
    );

    assert_eq!(
        SocketAddr::from_str("[2610:28:3090:3000:0:bad:cafe:47]:8080")
            .unwrap()
            .into_url(None)
            .unwrap()
            .as_str(),
        "http://[2610:28:3090:3000:0:bad:cafe:47]:8080/"
    );

    assert!(SocketAddr::from(([127, 0, 0, 1], 8080))
        .into_url(Some(&Url::parse("http://_").unwrap()))
        .is_err());
}

#[test]
fn ip_addr() {
    assert_eq!(
        IpAddr::from([127, 0, 0, 1])
            .into_url(None)
            .unwrap()
            .as_str(),
        "http://127.0.0.1/"
    );

    assert_eq!(
        IpAddr::from_str("::")
            .unwrap()
            .into_url(None)
            .unwrap()
            .as_str(),
        "http://[::]/"
    );

    assert_eq!(
        IpAddr::from_str("2610:28:3090:3000:0:bad:cafe:47")
            .unwrap()
            .into_url(None)
            .unwrap()
            .as_str(),
        "http://[2610:28:3090:3000:0:bad:cafe:47]/"
    );

    assert!(IpAddr::from([127, 0, 0, 1])
        .into_url(Some(&Url::parse("http://_").unwrap()))
        .is_err());
}
