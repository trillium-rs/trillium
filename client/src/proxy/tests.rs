use super::*;

fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}

fn proxy(endpoint: &str) -> Proxy {
    Proxy::new(endpoint).unwrap()
}

fn environment(values: &[(&str, &str)]) -> io::Result<Vec<Proxy>> {
    Proxy::from_env_with(|name| {
        Ok(values
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned()))
    })
}

fn selected(router: &impl ProxyRouter, origin: &str) -> Option<String> {
    router
        .route(&url(origin))
        .map(|proxy| proxy.endpoint().as_str().to_owned())
}

#[test]
fn schemes_select_origins() {
    let both = proxy("http://proxy.example:8080");
    let http = both.clone().with_schemes(ProxySchemes::Http);
    let https = both.clone().with_schemes(ProxySchemes::Https);
    for origin in ["http://origin.example", "https://origin.example"] {
        assert!(selected(&both, origin).is_some(), "{origin}");
    }
    assert!(selected(&http, "http://origin.example").is_some());
    assert!(selected(&http, "https://origin.example").is_none());
    assert!(selected(&https, "https://origin.example").is_some());
    assert!(selected(&https, "http://origin.example").is_none());
}

#[test]
fn function_routers_see_the_original_url_and_returned_bypass_entries_apply() {
    let router = |origin: &Url| {
        assert!(origin.as_str().ends_with("/path?q=1"));
        Some(
            proxy("http://proxy.example")
                .except(["bypass.example"])
                .unwrap(),
        )
    };
    assert!(selected(&router, "https://bypass.example/path?q=1").is_none());
    assert!(selected(&router, "https://origin.example/path?q=1").is_some());
}

#[test]
fn lists_take_the_first_proxy_that_carries_the_url() {
    let corporate = proxy("http://corporate.example")
        .except([".partner.example"])
        .unwrap();
    let partner = proxy("http://partner-gateway.example");
    let array = [corporate.clone(), partner.clone()];
    let list = vec![corporate, partner];
    let host = |router: &dyn ProxyRouter, origin: &str| {
        router
            .route(&url(origin))
            .map(|proxy| proxy.endpoint().host_str().unwrap().to_owned())
    };
    for router in [&array as &dyn ProxyRouter, &list] {
        assert_eq!(
            host(router, "https://origin.example").as_deref(),
            Some("corporate.example")
        );
        assert_eq!(
            host(router, "https://api.partner.example").as_deref(),
            Some("partner-gateway.example")
        );
    }
    assert!(selected(&Vec::<Proxy>::new(), "https://origin.example").is_none());
    assert!(selected(&None::<Proxy>, "https://origin.example").is_none());
}

#[test]
fn environment_builds_the_equivalent_list() {
    let shape = |proxies: Vec<Proxy>| {
        proxies
            .iter()
            .map(|proxy| (proxy.endpoint().as_str().to_owned(), proxy.schemes()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shape(
            environment(&[
                ("http_proxy", "plain.example:8080"),
                ("https_proxy", "http://secure.example"),
            ])
            .unwrap()
        ),
        [
            ("http://plain.example:8080/".to_owned(), ProxySchemes::Http),
            ("http://secure.example/".to_owned(), ProxySchemes::Https),
        ]
    );
    assert_eq!(
        shape(environment(&[("all_proxy", "http://fallback.example")]).unwrap()),
        [("http://fallback.example/".to_owned(), ProxySchemes::Both)]
    );
    assert_eq!(
        shape(
            environment(&[
                ("https_proxy", "http://secure.example"),
                ("all_proxy", "http://fallback.example"),
            ])
            .unwrap()
        ),
        [
            ("http://fallback.example/".to_owned(), ProxySchemes::Http),
            ("http://secure.example/".to_owned(), ProxySchemes::Https),
        ]
    );
    assert!(environment(&[]).unwrap().is_empty());
}

#[test]
fn env_scheme_all_fallback_and_lowercase_precedence() {
    let proxies = environment(&[
        ("http_proxy", "plain.example:8080"),
        ("https_proxy", "http://lower.example"),
        ("HTTPS_PROXY", "http://upper.example"),
        ("all_proxy", "http://fallback.example"),
        ("ALL_PROXY", "http://ignored.example"),
    ])
    .unwrap();
    assert_eq!(
        selected(&proxies, "http://origin.example").unwrap(),
        "http://plain.example:8080/"
    );
    assert_eq!(
        selected(&proxies, "https://origin.example").unwrap(),
        "http://lower.example/"
    );
    let proxies = environment(&[
        ("http_proxy", ""),
        ("HTTP_PROXY", "http://ignored.example"),
        ("ALL_PROXY", "http://fallback.example"),
    ])
    .unwrap();
    assert_eq!(
        selected(&proxies, "http://origin.example").unwrap(),
        "http://fallback.example/"
    );
    let proxies = environment(&[
        ("https_proxy", ""),
        ("HTTPS_PROXY", "http://ignored.example"),
    ])
    .unwrap();
    assert!(selected(&proxies, "https://origin.example").is_none());
}

#[test]
fn unused_all_proxy_is_not_parsed() {
    // A common Clash/Surge shape: a SOCKS fallback nobody needs.
    let proxies = environment(&[
        ("http_proxy", "proxy.example:8080"),
        ("https_proxy", "secure.example:8080"),
        ("all_proxy", "socks5://fallback.example:1080"),
        ("ALL_PROXY", "socks5://ignored.example:1080"),
    ])
    .unwrap();
    assert_eq!(
        selected(&proxies, "http://origin.example").unwrap(),
        "http://proxy.example:8080/"
    );
    assert_eq!(
        selected(&proxies, "https://origin.example").unwrap(),
        "http://secure.example:8080/"
    );
}

#[test]
fn unsupported_all_proxy_fails_only_when_a_scheme_needs_it() {
    for values in [
        &[("http_proxy", "proxy.example:8080")][..],
        &[("https_proxy", "secure.example:8080")][..],
        &[][..],
    ] {
        let values = values
            .iter()
            .copied()
            .chain([("all_proxy", "socks5://fallback.example")])
            .collect::<Vec<_>>();
        let error = environment(&values).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().starts_with("all_proxy: "), "{error}");
    }
}

#[test]
fn uppercase_http_proxy_is_ignored_under_cgi() {
    assert_eq!(
        selected(
            &environment(&[("HTTP_PROXY", "http://upper.example")]).unwrap(),
            "http://origin.example"
        )
        .as_deref(),
        Some("http://upper.example/")
    );
    let proxies = Proxy::from_env_with(|name| {
        assert_ne!(
            name, "HTTP_PROXY",
            "a CGI request header must not be read as a proxy"
        );
        Ok((name == "REQUEST_METHOD").then(|| "GET".to_owned()))
    })
    .unwrap();
    assert!(selected(&proxies, "http://origin.example").is_none());
}

#[test]
fn env_no_proxy_and_empty_lowercase_override() {
    let proxies = environment(&[
        ("all_proxy", "http://proxy.example"),
        ("NO_PROXY", "origin.example,127.0.0.0/8"),
    ])
    .unwrap();
    assert!(selected(&proxies, "https://sub.origin.example").is_none());
    assert!(selected(&proxies, "http://127.0.0.1").is_none());
    assert!(selected(&proxies, "https://other.example").is_some());
    let proxies = environment(&[
        ("all_proxy", "http://proxy.example"),
        ("no_proxy", ""),
        ("NO_PROXY", "*"),
    ])
    .unwrap();
    assert!(selected(&proxies, "https://origin.example").is_some());
}

#[test]
fn wildcard_no_proxy_turns_proxying_off_without_parsing_proxies() {
    for values in [
        &[
            ("http_proxy", "http://proxy.example"),
            ("no_proxy", "localhost, *"),
        ][..],
        &[
            ("http_proxy", "socks5://unparsed.example"),
            ("NO_PROXY", "*"),
        ][..],
    ] {
        assert!(environment(values).unwrap().is_empty());
    }
}

#[test]
fn env_credentials_are_decoded_and_redacted() {
    let proxies = environment(&[(
        "https_proxy",
        "http://user%40example:p%3Aa%20ss@proxy.example:8080",
    )])
    .unwrap();
    let proxy = proxies.route(&url("https://origin.example")).unwrap();
    assert_eq!(proxy.endpoint().as_str(), "http://proxy.example:8080/");
    assert_eq!(
        proxy
            .connect_headers()
            .get_str(KnownHeaderName::ProxyAuthorization),
        Some("Basic dXNlckBleGFtcGxlOnA6YSBzcw==")
    );
    let debug = format!("{proxies:?} {proxy:?}");
    for secret in ["dXNlck", "user@example", "user%40example"] {
        assert!(!debug.contains(secret), "{debug}");
    }
}

#[test]
fn malformed_environment_is_an_error_not_a_direct_route() {
    for (variable, value) in [
        ("http_proxy", "socks5://proxy.example"),
        ("https_proxy", "http://[broken"),
        ("HTTPS_PROXY", "http://a%3Ab:secret@proxy.example"),
    ] {
        let error = environment(&[(variable, value)]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            error.to_string().starts_with(&format!("{variable}: ")),
            "{error}"
        );
        // Proxy values can carry credentials: name the variable, never echo the value.
        assert!(!error.to_string().contains(value), "{error}");
        assert!(!error.to_string().contains("secret"), "{error}");
    }
    let error = environment(&[("no_proxy", "good.example, host:notaport")]).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(
        error.to_string(),
        "no_proxy: proxy bypass entry \"host:notaport\": invalid port"
    );
    let error = Proxy::from_env_with(|_| Err(invalid("not Unicode"))).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn bypass_domains_respect_label_boundaries_and_normalize() {
    let bypass = NoProxy::new(".EXAMPLE.com., *.other.example, bücher.example,,").unwrap();
    for origin in [
        "http://example.com",
        "https://a.example.com",
        "http://example.com.",
        "http://other.example",
        "http://a.other.example",
        "http://xn--bcher-kva.example",
    ] {
        assert!(bypass.contains(&url(origin)), "{origin}");
    }
    for origin in [
        "http://notexample.com",
        "http://example.com.evil",
        "http://otherexample",
        "http://127.0.0.1",
    ] {
        assert!(!bypass.contains(&url(origin)), "{origin}");
    }
}

#[test]
fn bypass_ports_and_literal_ip_networks() {
    let bypass =
        NoProxy::new("example.com:443, 127.0.0.0/8, [::1]:8080, 2001:db8::/32, 192.0.2.1:80")
            .unwrap();
    for origin in [
        "https://example.com",
        "http://example.com:443",
        "http://127.123.1.2",
        "http://[::1]:8080",
        "https://[2001:db8::1234]",
        "http://192.0.2.1",
    ] {
        assert!(bypass.contains(&url(origin)), "{origin}");
    }
    for origin in [
        "http://example.com",
        "http://localhost",
        "http://[::1]",
        "http://[2001:db9::1]",
        "https://192.0.2.1",
    ] {
        assert!(!bypass.contains(&url(origin)), "{origin}");
    }
}

#[test]
fn invalid_bypass_entries_are_rejected() {
    for entry in [
        "http://example.com",
        "host:65536",
        "host:-1",
        "host:",
        "[::1",
        "[::1]suffix",
        "*evil.example",
        "192.0.2.0/99",
        ".",
        "a/b",
        "*",
        "",
        " localhost",
        "localhost ",
        "localhost,internal.example",
    ] {
        assert!(
            proxy("http://proxy.example").except([entry]).is_err(),
            "{entry}"
        );
    }
    assert!(!NoProxy::default().contains(&url("http://origin.example")));
}

#[test]
fn endpoints_require_http_and_convert_credentials() {
    for value in ["socks5://proxy.example:1080", "file:///proxy"] {
        assert_eq!(
            Proxy::new(value).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    assert!(Proxy::new("https://proxy.example").is_ok());
    assert_eq!(
        proxy("proxy.example:8080").endpoint().as_str(),
        "http://proxy.example:8080/"
    );
    let credentialed = proxy("http://user:secret@proxy.example:8080");
    assert_eq!(
        credentialed.endpoint().as_str(),
        "http://proxy.example:8080/"
    );
    assert_eq!(
        credentialed
            .connect_headers()
            .get_str(KnownHeaderName::ProxyAuthorization),
        Some("Basic dXNlcjpzZWNyZXQ=")
    );
}
