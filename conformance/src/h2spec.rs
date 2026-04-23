//! h2spec subcommand: run [h2spec](https://github.com/summerwind/h2spec) against a live
//! trillium HTTP/2 server and diff the JUnit results against the tracked pass-set.

use crate::{Runtime, Tls, server};
use anyhow::{Context, bail};
use junit_parser::TestStatus;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// CLI args for `conformance h2spec`.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Runtime adapter to run the server on.
    #[arg(long, value_enum, default_value_t = Runtime::Tokio)]
    pub runtime: Runtime,

    /// TLS configuration for the server. `none` runs h2 cleartext (prior knowledge).
    #[arg(long, value_enum, default_value_t = Tls::None)]
    pub tls: Tls,

    /// Per-test timeout h2spec passes to its subprocess in seconds. Short default keeps the
    /// whole run under a minute.
    #[arg(long, default_value_t = 3)]
    pub per_test_timeout: u64,

    /// Emit the JUnit XML to this path instead of a generated tempfile; useful for CI.
    #[arg(long)]
    pub junit_out: Option<PathBuf>,

    /// Fail with a nonzero exit if h2spec reports tests passing that aren't in the pass-set.
    /// Default: report them on stderr but exit 0.
    #[arg(long)]
    pub strict_unexpected_passes: bool,
}

/// Entry point for `conformance h2spec ...`.
pub fn run(args: Args) -> anyhow::Result<()> {
    ensure_h2spec_installed()?;
    let summary = run_one(&args)?;
    print_summary(&summary);
    if !summary.regressions.is_empty() {
        bail!("{} h2spec regression(s)", summary.regressions.len());
    }
    if args.strict_unexpected_passes && !summary.unexpected_passes.is_empty() {
        bail!(
            "{} h2spec unexpected pass(es) with --strict-unexpected-passes",
            summary.unexpected_passes.len()
        );
    }
    Ok(())
}

/// Entry point for `conformance all` — run h2spec across the runtime × TLS grid, report per
/// cell, exit nonzero if any cell regresses.
pub fn run_all() -> anyhow::Result<()> {
    ensure_h2spec_installed()?;
    let mut any_regression = false;
    for runtime in [Runtime::Tokio, Runtime::Smol, Runtime::AsyncStd] {
        for tls in [Tls::None, Tls::Rustls] {
            println!("=== h2spec | runtime={runtime:?} tls={tls:?} ===");
            let args = Args {
                runtime,
                tls,
                per_test_timeout: 3,
                junit_out: None,
                strict_unexpected_passes: false,
            };
            match run_one(&args) {
                Ok(summary) => {
                    print_summary(&summary);
                    if !summary.regressions.is_empty() {
                        any_regression = true;
                    }
                }
                Err(e) => {
                    eprintln!("{runtime:?}/{tls:?}: {e:#}");
                    any_regression = true;
                }
            }
            println!();
        }
    }
    if any_regression {
        bail!("one or more cells had regressions");
    }
    Ok(())
}

/// Verify h2spec binary is on PATH — pointer to install docs is more useful than a raw
/// "command not found."
fn ensure_h2spec_installed() -> anyhow::Result<()> {
    let status = Command::new("h2spec")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => bail!(
            "`h2spec` binary not found on PATH. Install from https://github.com/summerwind/h2spec \
             (`brew install h2spec` on macOS)."
        ),
    }
}

#[derive(Debug)]
struct Summary {
    runtime: Runtime,
    tls: Tls,
    total: usize,
    passed: usize,
    regressions: Vec<String>,
    unexpected_passes: Vec<String>,
    missing_from_results: Vec<String>,
}

fn run_one(args: &Args) -> anyhow::Result<Summary> {
    let handle = server::start(args.runtime, args.tls)?;
    let addr = handle.addr;
    log::info!(
        "conformance server bound at {addr} (runtime={:?}, tls={:?})",
        args.runtime,
        args.tls
    );

    // Give the server a beat to finish any asynchronous post-bind setup (rustls acceptor
    // wiring etc). Without this we occasionally race h2spec's first connect.
    std::thread::sleep(Duration::from_millis(50));

    let junit_path = args.junit_out.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "trillium-h2spec-{}-{}.xml",
            std::process::id(),
            addr.port()
        ))
    });
    let junit_arg = junit_path.to_str().context("junit path is not utf-8")?;

    let mut cmd = Command::new("h2spec");
    cmd.args([
        "--host",
        &addr.ip().to_string(),
        "-p",
        &addr.port().to_string(),
        "-j",
        junit_arg,
        "-o",
        &args.per_test_timeout.to_string(),
    ]);
    if args.tls == Tls::Rustls {
        cmd.args(["--tls", "--insecure"]);
    }

    let status = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to spawn h2spec")?;
    let _ = status; // h2spec's exit reflects test results; we read the JUnit report instead.

    handle.shut_down();

    let summary = parse_and_diff(&junit_path, args.runtime, args.tls)?;
    if args.junit_out.is_none() {
        let _ = fs::remove_file(&junit_path);
    }
    Ok(summary)
}

fn parse_and_diff(junit_path: &Path, runtime: Runtime, tls: Tls) -> anyhow::Result<Summary> {
    let xml = fs::read_to_string(junit_path).with_context(|| {
        format!(
            "h2spec produced no JUnit report at {}",
            junit_path.display()
        )
    })?;
    let suites = junit_parser::from_reader(xml.as_bytes()).context("parse JUnit XML")?;

    let pass_set = load_pass_set()?;
    let mut seen = HashSet::new();
    let mut regressions = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut total = 0;
    let mut passed = 0;

    for suite in &suites.suites {
        // h2spec encodes the section (`http2/6.7`, `hpack/5.2`, ...) as the testsuite's
        // non-standard `package` attribute and the human-readable summary as each
        // testcase's `classname` attribute. Stitch `{suite.package} / {case.classname}` to
        // form stable identifiers matching the pass-set format.
        let section = suite.package.as_deref().unwrap_or(&suite.name);
        for case in &suite.cases {
            total += 1;
            let classname = case.classname.as_deref().unwrap_or(&case.name);
            let id = format!("{section} / {classname}");
            seen.insert(id.clone());
            let is_pass = matches!(case.status, TestStatus::Success);
            if is_pass {
                passed += 1;
            }
            match (pass_set.contains(&id), is_pass) {
                (true, true) | (false, false) => {}
                (true, false) => regressions.push(id),
                (false, true) => unexpected_passes.push(id),
            }
        }
    }

    let missing_from_results: Vec<String> = pass_set.difference(&seen).cloned().collect();

    regressions.sort();
    unexpected_passes.sort();
    let mut missing = missing_from_results;
    missing.sort();

    Ok(Summary {
        runtime,
        tls,
        total,
        passed,
        regressions,
        unexpected_passes,
        missing_from_results: missing,
    })
}

fn load_pass_set() -> anyhow::Result<HashSet<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("h2spec-pass-set.txt");
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("h2spec pass-set not found at {}", path.display()))?;
    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect())
}

fn print_summary(s: &Summary) {
    println!(
        "runtime={:?} tls={:?}: {}/{} passing",
        s.runtime, s.tls, s.passed, s.total
    );
    if !s.regressions.is_empty() {
        println!("  regressions ({}):", s.regressions.len());
        for id in &s.regressions {
            println!("    - {id}");
        }
    }
    if !s.unexpected_passes.is_empty() {
        println!("  unexpected passes ({}):", s.unexpected_passes.len());
        for id in &s.unexpected_passes {
            println!("    + {id}");
        }
    }
    if !s.missing_from_results.is_empty() {
        println!(
            "  pass-set entries not in h2spec output (stale identifiers?): {}",
            s.missing_from_results.len()
        );
        for id in &s.missing_from_results {
            println!("    ? {id}");
        }
    }
}
