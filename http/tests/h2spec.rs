//! External-conformance test: runs the `h2spec` Go binary against a live `trillium-http` HTTP/2
//! listener and diffs the results against `tests/h2spec-pass-set.txt`.
//!
//! Gated behind the `h2-conformance` feature so the external binary isn't a hard requirement for
//! `cargo test`. Run with:
//!
//! ```text
//! cargo test -p trillium-http --features h2-conformance --test h2spec
//! ```
//!
//! Failure modes:
//! - any test listed in the pass-set that h2spec reports as failed → **regression** (hard fail).
//! - any test h2spec reports as passed that isn't in the pass-set → **unexpected pass** (logged;
//!   caller is expected to add it to the file).
#![cfg(feature = "h2-conformance")]

use async_compat::Compat;
use std::{
    collections::HashSet,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
};
use tokio::net::TcpListener;
use trillium_http::{HttpContext, h2::H2Connection};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2spec_conformance() {
    let _ = env_logger::try_init();

    if Command::new("h2spec")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        panic!(
            "`h2spec` binary not found on PATH. Install from https://github.com/summerwind/h2spec \
             or disable the `h2-conformance` feature."
        );
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let context = Arc::new(HttpContext::default());
    let accept_context = context.clone();
    let accept_task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let _ = stream.set_nodelay(true);
            let conn = H2Connection::new(accept_context.clone());
            tokio::spawn(async move {
                let mut acceptor = conn.run(Compat::new(stream));
                loop {
                    match acceptor.next().await {
                        Ok(None) | Err(_) => break,
                        Ok(Some(_transport)) => {
                            // Phase-3 work in progress: streams are emitted but the
                            // H2Transport's read/write are stubs. Drop it; h2spec gets a stalled
                            // stream which suffices for a number of stream-state tests.
                        }
                    }
                }
            });
        }
    });

    let xml_path = std::env::temp_dir().join(format!(
        "trillium-h2spec-{}-{}.xml",
        std::process::id(),
        port
    ));

    let xml_path_for_spawn = xml_path.clone();
    let port_for_spawn = port;
    let h2spec_status = tokio::task::spawn_blocking(move || {
        Command::new("h2spec")
            .args([
                "-p",
                &port_for_spawn.to_string(),
                "-j",
                xml_path_for_spawn.to_str().unwrap(),
                "-o",
                "3",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    })
    .await
    .expect("spawn_blocking joined")
    .expect("h2spec spawn failed");

    // h2spec's exit code reflects test results, not runner health. The JUnit report is what we
    // compare against the pass-set, so we don't assert on the status directly.
    let _ = h2spec_status;

    // `listener.accept()` would otherwise block forever waiting for a connection that never
    // comes. Aborting is safe: h2spec has already closed its last connection, and any in-flight
    // H2Connection tasks wind down via the HttpContext swansong below.
    accept_task.abort();
    context.shut_down();

    let xml = std::fs::read_to_string(&xml_path).expect("h2spec produced no JUnit report");
    let _ = std::fs::remove_file(&xml_path);

    let results = parse_junit(&xml);
    assert!(!results.is_empty(), "h2spec reported zero test cases");

    let pass_set_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/h2spec-pass-set.txt");
    let pass_set: HashSet<String> = std::fs::read_to_string(&pass_set_path)
        .expect("tests/h2spec-pass-set.txt missing")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();

    let mut regressions = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut missing_from_results = pass_set.clone();

    for (id, outcome) in &results {
        missing_from_results.remove(id);
        match (pass_set.contains(id), outcome) {
            (true, Outcome::Pass) => {}
            (true, Outcome::Fail) => regressions.push(id.clone()),
            (false, Outcome::Pass) => unexpected_passes.push(id.clone()),
            (false, Outcome::Fail) => {}
        }
    }

    if !unexpected_passes.is_empty() {
        unexpected_passes.sort();
        eprintln!(
            "h2spec unexpected passes — consider adding to tests/h2spec-pass-set.txt:\n  {}",
            unexpected_passes.join("\n  ")
        );
    }

    let mut missing: Vec<_> = missing_from_results.into_iter().collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "pass-set entries not present in h2spec output (stale identifiers?): {missing:?}",
    );

    regressions.sort();
    assert!(
        regressions.is_empty(),
        "h2spec regressions (tests in pass-set that failed):\n  {}",
        regressions.join("\n  "),
    );
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    Pass,
    Fail,
}

/// Minimal JUnit-XML parser scoped to the tag shape h2spec produces:
/// `<testcase package="..." classname="..." time="...">` optionally containing `<failure ...>` or
/// `<error ...>` before `</testcase>`. No attribute escaping handling beyond basic quoted strings
/// — fine for h2spec test names (no embedded quotes).
///
/// Identifiers are formatted as `<package> / <classname>` since h2spec doesn't emit its own
/// per-test IDs; `package` encodes the RFC section (e.g. `http2/6.7`) and `classname` is the
/// per-test summary line. Together they're unique and self-documenting.
fn parse_junit(xml: &str) -> Vec<(String, Outcome)> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(rel) = xml[cursor..].find("<testcase ") {
        let tag_start = cursor + rel;
        let Some(tag_end_rel) = xml[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + tag_end_rel;
        let tag = &xml[tag_start..=tag_end];

        let package = extract_attr(tag, "package").unwrap_or("");
        let classname = extract_attr(tag, "classname").unwrap_or("");
        let id = format!("{package} / {classname}");

        let close_rel = xml[tag_end..]
            .find("</testcase>")
            .expect("unterminated <testcase> in JUnit output");
        let case_end = tag_end + close_rel;
        let body = &xml[tag_end..case_end];
        let outcome = if body.contains("<failure") || body.contains("<error") {
            Outcome::Fail
        } else {
            Outcome::Pass
        };
        out.push((id, outcome));
        cursor = case_end + "</testcase>".len();
    }
    out
}

fn extract_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}
