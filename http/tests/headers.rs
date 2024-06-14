use indoc::indoc;
use pretty_assertions::{assert_eq, assert_str_eq};
use test_harness::test;
use trillium_http::{
    Headers,
    KnownHeaderName::{self, ContentLength},
};

#[test]
fn known_entry() {
    let mut headers = Headers::new();
    let header_name = ContentLength;
    let entry = headers.entry(header_name);
    assert!(entry.is_vacant());
    assert!(!entry.is_occupied());

    assert_eq!(entry.name(), header_name);

    assert_eq!(
        "Vacant(VacantEntry { name: ContentLength })",
        format!("{entry:?}")
    );

    entry.insert("value");

    let entry = headers.entry(header_name);
    assert_eq!(entry.name(), header_name);

    assert_str_eq!(headers.get_str(header_name).unwrap(), "value");
    assert_eq!(headers.entry(header_name).or_insert("ignored"), "value");
    assert_str_eq!(
        r#"Occupied(OccupiedEntry { name: ContentLength, values: "value" })"#,
        format!("{:?}", headers.entry(header_name))
    );

    assert_eq!(headers.entry(header_name).insert("new-value"), "new-value");
    assert_str_eq!(headers.get_str(header_name).unwrap(), "new-value");

    headers.remove(header_name);
    assert!(!headers.has_header(header_name));
    assert_eq!(
        headers
            .entry(header_name)
            .or_insert_with(|| String::from("generated-value")),
        "generated-value"
    );

    assert_eq!(
        **headers.entry(header_name).append("appended-header-value"),
        ["generated-value", "appended-header-value"]
    );

    headers.remove(header_name);
    assert_eq!(
        **headers.entry(header_name).append("appended-header-value"),
        ["appended-header-value"]
    );

    let occupied = headers.entry(header_name).occupied().unwrap();
    assert_eq!(occupied.remove(), "appended-header-value");

    headers.insert(header_name, "some-value");
    let mut occupied = headers.entry(header_name).occupied().unwrap();
    occupied.append("another-value");
    let (n, v) = occupied.remove_entry();
    assert_eq!(n, header_name);
    assert_eq!(*v, ["some-value", "another-value"]);
}

#[test]
fn unknown_entry() {
    let mut headers = Headers::new();
    let header_name = "x-unknown-header";
    let entry = headers.entry(header_name);
    assert!(entry.is_vacant());
    assert!(!entry.is_occupied());
    assert_eq!(entry.name(), header_name);
    let entry = entry.and_modify(|_| panic!("never called"));
    assert!(entry.occupied().is_none());
    assert!(headers.entry(header_name).vacant().is_some());
    let entry = headers.entry(header_name);

    assert_str_eq!(
        r#"Vacant(VacantEntry { name: "x-unknown-header" })"#,
        format!("{entry:?}")
    );

    entry.insert("value");

    let entry = headers.entry(header_name);
    assert!(!entry.is_vacant());
    assert!(entry.is_occupied());

    assert!(entry.occupied().is_some());
    assert!(headers.entry(header_name).vacant().is_none());

    assert_str_eq!(headers.get_str(header_name).unwrap(), "value");
    assert_eq!(headers.entry(header_name).or_insert("ignored"), "value");
    assert_str_eq!(
        r#"Occupied(OccupiedEntry { name: "x-unknown-header", values: "value" })"#,
        format!("{:?}", headers.entry(header_name))
    );

    let entry = headers.entry(header_name);
    assert_eq!(entry.name(), header_name);

    assert_eq!(headers.entry(header_name).insert("new-value"), "new-value");
    assert_str_eq!(headers.get_str(header_name).unwrap(), "new-value");

    headers.remove(header_name);
    assert!(!headers.has_header(header_name));
    assert_eq!(
        headers
            .entry(header_name)
            .or_insert_with(|| String::from("generated-value")),
        "generated-value"
    );

    assert_eq!(
        **headers.entry(header_name).append("appended-header-value"),
        ["generated-value", "appended-header-value"]
    );

    assert_eq!(
        **headers
            .entry(header_name)
            .and_modify(|values| values.sort())
            .or_insert(""),
        ["appended-header-value", "generated-value"]
    );

    headers.remove(header_name);
    assert_eq!(
        **headers.entry(header_name).append("appended-header-value"),
        ["appended-header-value"]
    );

    let occupied = headers.entry(header_name).occupied().unwrap();
    assert_eq!(occupied.remove(), "appended-header-value");

    headers.insert(header_name, "some-value");
    let mut occupied = headers.entry(header_name).occupied().unwrap();
    occupied.append("another-value");
    let (n, v) = occupied.remove_entry();
    assert_eq!(n, header_name);
    assert_eq!(*v, ["some-value", "another-value"]);
}

#[test]
fn headers_known() {
    let mut headers = Headers::new();
    let header_name = ContentLength;
    assert!(headers.is_empty());
    assert_eq!(headers.len(), 0);
    assert!(!headers.has_header(header_name));

    headers.insert(header_name, 100);
    assert!(!headers.is_empty());
    assert_eq!(headers.len(), 1);
    assert!(headers.has_header(header_name));

    assert_str_eq!("Content-Length: 100\r\n", format!("{headers}"));
    assert_str_eq!(
        r#"Headers { known: {ContentLength: "100"}, unknown: {} }"#,
        format!("{headers:?}")
    );
    assert_eq!(**headers.get_values(header_name).unwrap(), ["100"]);
    headers.try_insert(header_name, "ignored");
    assert_eq!(**headers.get_values(header_name).unwrap(), ["100"]);

    headers.append(header_name, "second value");
    assert!(!headers.is_empty());
    assert_eq!(headers.len(), 1);

    assert_str_eq!(
        "Content-Length: 100\r\nContent-Length: second value\r\n",
        format!("{headers}")
    );
    assert_str_eq!(
        r#"Headers { known: {ContentLength: ["100", "second value"]}, unknown: {} }"#,
        format!("{headers:?}")
    );
    assert_eq!(
        **headers.get_values(header_name).unwrap(),
        ["100", "second value"]
    );

    headers.try_insert(header_name, "ignored");
    assert_eq!(
        **headers.get_values(header_name).unwrap(),
        ["100", "second value"]
    );

    headers.remove(header_name);
    headers.try_insert(header_name, "INSERTED");
    assert_eq!(headers.get_values(header_name).unwrap(), "INSERTED");
    assert_str_eq!(headers.get_str(header_name).unwrap(), "INSERTED");
    assert!(headers.eq_ignore_ascii_case(header_name, "inserted"));
    assert!(!headers.eq_ignore_ascii_case(header_name, "other"));
}

#[test]
fn bulk_header_operations() {
    let headers = Headers::from_iter([("Content-Length", 1), ("x-unknown-header", 2)])
        .with_inserted_header("x-other", 1)
        .with_inserted_header("other-Header", format_args!("1 + 2 = {}", 1 + 2))
        .with_inserted_header(KnownHeaderName::Host, "host")
        .with_inserted_header(KnownHeaderName::Host, "other-host")
        .with_appended_header(KnownHeaderName::Server, String::from("server"))
        .with_appended_header(KnownHeaderName::Server, "x")
        .without_header("x-Unknown-Header")
        .without_header(ContentLength);

    assert_str_eq!(
        headers.to_string(),
        "Host: other-host\r\nServer: server\r\nServer: x\r\nother-Header: 1 + 2 = 3\r\nx-other: \
         1\r\n"
    );

    assert_str_eq!(
        headers
            .without_headers(["x-unknown-header", "server"])
            .to_string(),
        "Host: other-host\r\nother-Header: 1 + 2 = 3\r\nx-other: 1\r\n"
    );
}

#[test]
fn combining_headers() {
    let headers_a = Headers::from_iter([
        ("a", "b"),
        ("c", "d"),
        ("host", "is a known header"),
        ("server", "known"),
    ]);
    let headers_b = Headers::from_iter([
        ("A", "E"),
        ("C", "F"),
        ("HOST", "also known"),
        ("SERVER", "also known"),
        ("new-unknown", "only in b"),
        ("Content-TYPE", "also only in b"),
    ]);

    let mut extended = headers_a.clone();
    extended.extend(headers_b.clone());
    assert_str_eq!(
        indoc! {"
            Host: is a known header\r
            Host: also known\r
            Content-Type: also only in b\r
            Server: known\r
            Server: also known\r
            new-unknown: only in b\r
            a: b\r
            a: E\r
            c: d\r
            c: F\r
        "},
        extended.to_string(),
    );

    let mut insert_all = headers_a.clone();
    insert_all.insert_all(headers_b.clone());
    assert_str_eq!(
        indoc! {"
            Host: also known\r
            Content-Type: also only in b\r
            Server: also known\r
            c: F\r
            new-unknown: only in b\r
            a: E\r
        "},
        insert_all.to_string(),
    );

    let mut append_all = headers_a.clone();
    append_all.append_all(headers_b.clone());
    assert_str_eq!(
        indoc! {"
            Host: is a known header\r
            Host: also known\r
            Content-Type: also only in b\r
            Server: known\r
            Server: also known\r
            new-unknown: only in b\r
            a: b\r
            a: E\r
            c: d\r
            c: F\r
        "},
        append_all.to_string(),
    );
}

#[test]
fn headers_unknown() {
    let mut headers = Headers::new();
    let header_name = "x-unknown-header";
    assert!(headers.is_empty());
    assert_eq!(headers.len(), 0);
    assert!(!headers.has_header(header_name));

    headers.insert(header_name, 100);
    assert!(!headers.is_empty());
    assert_eq!(headers.len(), 1);
    assert!(headers.has_header(header_name));

    assert_str_eq!("x-unknown-header: 100\r\n", format!("{headers}"));
    assert_str_eq!(
        r#"Headers { known: {}, unknown: {"x-unknown-header": "100"} }"#,
        format!("{headers:?}")
    );
    assert_eq!(**headers.get_values(header_name).unwrap(), ["100"]);
    headers.try_insert(header_name, "ignored");
    assert_eq!(**headers.get_values(header_name).unwrap(), ["100"]);

    headers.append(header_name, "second value");
    assert!(!headers.is_empty());
    assert_eq!(headers.len(), 1);

    assert_str_eq!(
        "x-unknown-header: 100\r\nx-unknown-header: second value\r\n",
        format!("{headers}")
    );
    assert_str_eq!(
        r#"Headers { known: {}, unknown: {"x-unknown-header": ["100", "second value"]} }"#,
        format!("{headers:?}")
    );
    assert_eq!(
        **headers.get_values(header_name).unwrap(),
        ["100", "second value"]
    );

    headers.try_insert(header_name, "ignored");
    assert_eq!(
        **headers.get_values(header_name).unwrap(),
        ["100", "second value"]
    );

    headers.remove(header_name);
    headers.try_insert(header_name, "INSERTED");
    assert_eq!(headers.get_values(header_name).unwrap(), "INSERTED");
    assert_str_eq!(headers.get_str(header_name).unwrap(), "INSERTED");
    assert!(headers.eq_ignore_ascii_case(header_name, "inserted"));
    assert!(!headers.eq_ignore_ascii_case(header_name, "other"));

    headers.remove(header_name);
    headers.append(header_name, "inserted");
    assert_str_eq!(headers.get_str(header_name).unwrap(), "inserted");
}

#[test]
fn header_names_are_case_insensitive_for_access_but_retain_initial_case_in_headers() {
    let mut headers = Headers::new();
    headers.insert("my-Header-name", "initial-value");
    headers.insert("my-Header-NAME", "my-header-value");

    assert_eq!(headers.len(), 1);

    assert_eq!(
        headers.get_str("My-Header-Name").unwrap(),
        "my-header-value"
    );

    headers.append("mY-hEaDer-NaMe", "second-value");
    assert_eq!(
        headers.get_values("my-header-name").unwrap(),
        ["my-header-value", "second-value"].as_slice()
    );

    assert_eq!(
        headers.iter().next().unwrap().0.to_string(),
        "my-Header-name"
    );

    assert!(headers.remove("my-HEADER-name").is_some());
    assert!(headers.is_empty());
}

#[test]
fn value_case_insensitive_comparison() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::Upgrade, "WebSocket");
    headers.insert(KnownHeaderName::Connection, "upgrade");

    assert!(headers.eq_ignore_ascii_case(KnownHeaderName::Upgrade, "websocket"));
    assert!(headers.eq_ignore_ascii_case(KnownHeaderName::Connection, "Upgrade"));
}
