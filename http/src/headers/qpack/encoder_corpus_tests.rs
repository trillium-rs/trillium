//! Encoder-side QPACK corpus test.
//!
//! Symmetric to [`super::decoder_corpus_tests`], but exercises our *encoder* rather than our
//! decoder. For every `.qif` file in the `tests/qifs/qifs/` corpus, at several `(capacity,
//! max_blocked_streams)` configs, runs the groups through the full encode → drain
//! encoder-stream → decode pipeline and compares the decoded output against the original qif.
//!
//! ## What this tests
//!
//! * Encoder correctness across a wide input distribution (thousands of realistic request and
//!   response groups from the qpack-interop corpus).
//! * Encoder/decoder contract: our encoder's output is parseable by our decoder, including
//!   dynamic-table entries inserted on the encoder stream.
//! * Eviction, pinning, and blocked-streams budget enforcement under sustained use.
//!
//! ## What this does *not* test
//!
//! * Byte-for-byte equivalence with reference encoders — different encoders make different (all
//!   legal) choices. The compression metric below measures relative size; it is not a correctness
//!   assertion.
//! * Cross-implementation interop — our encoder against a reference decoder. That would require
//!   shelling out to the offline-interop tool; it is out of scope for unit tests.
//!
//! ## Ack policy
//!
//! The test simulates an "always-ack" peer: after each successful decode, we call
//! [`EncoderDynamicTable::on_section_ack`] on the stream if the encoded section had a
//! non-zero Required Insert Count. This is deliberate — the metric substrate must be stable
//! for the compression numbers to be a useful baseline as policy changes are made. A
//! randomized-ack variant (fastrand-seeded, high-probability ack) is a reasonable follow-up
//! once this test is known to pass, but must stay out of the compression-metric path.
//!
//! ## Compression metric
//!
//! Setting `QPACK_ENCODER_STATS=1` opts in to a post-test summary that tabulates our total
//! encoded bytes against the matching reference-encoder outputs in
//! `encoded/qpack-06/{encoder}/{stem}.out.{cap}.{blocked}.1` (variant 1 = immediate acks,
//! which matches our always-ack substrate). The comparison is an informational dashboard —
//! no assertion.
//!
//! ## Filter
//!
//! `QPACK_ENCODER_CORPUS_FILTER=substring` restricts the run to qif files whose path contains
//! that substring. Mirrors `QPACK_CORPUS_FILTER` on the decoder side.

use super::{DecoderDynamicTable, EncoderDynamicTable, qif};
use crate::h3::H3Settings;
use futures_lite::{future, io::Cursor};
use std::{collections::BTreeMap, path::Path};

// `Headers` iterates unknown names through a `HashMap`, so encoding from a `FieldSection`
// produces non-deterministic wire bytes when a group contains multiple unknown headers (the
// dynamic table gets populated in random order, which then ripples into every subsequent
// section's references). The corpus test bypasses `FieldSection` entirely and feeds qif-file
// order to [`EncoderDynamicTable::encode_field_lines`] so the compression metric is a stable
// baseline that policy changes can be measured against.

/// A `(max_capacity, max_blocked_streams)` pair. `u64` to match `H3Settings` units.
#[derive(Debug, Clone, Copy)]
struct Config {
    capacity: u64,
    max_blocked: u64,
}

impl Config {
    const fn new(capacity: u64, max_blocked: u64) -> Self {
        Self {
            capacity,
            max_blocked,
        }
    }
}

/// Configs run against every qif file. Mirrors the subset of the offline-interop matrix we
/// most care about:
///
/// * `(0, 0)` — static-table only. Establishes the literal-fallback floor; when the policy evolves
///   to use the dynamic table this number should stay identical (it's the config where the dynamic
///   table is inert).
/// * `(256, 100)` — small table with blocking budget. Exercises eviction on realistic inputs.
/// * `(4096, 0)` — dynamic table present but blocking-streams budget is zero. With the current
///   insert-then-reference policy this degenerates to no dynamic-table usage (every insert-then-ref
///   is blocking at emit time, and the zero-budget gate rejects it). Worth testing anyway: guards
///   against regressions that would cheat the budget, and becomes meaningful once an
///   ack-then-reference strategy lands.
/// * `(4096, 100)` — typical real-world config; exercises the full planner.
const CONFIGS: &[Config] = &[
    Config::new(0, 0),
    Config::new(256, 100),
    Config::new(4096, 0),
    Config::new(4096, 100),
];

// --- Stats ---

/// Per-config byte totals for one qif file, summed across every group.
#[derive(Debug, Default, Clone, Copy)]
struct EncodeStats {
    /// Bytes emitted as field-section output from `encode()` across all groups.
    section_bytes: usize,
    /// Bytes drained from the encoder stream (Set Dynamic Table Capacity + Inserts +
    /// Duplicates).
    encoder_stream_bytes: usize,
}

impl EncodeStats {
    fn total(&self) -> usize {
        self.section_bytes + self.encoder_stream_bytes
    }
}

// --- Core runner ---

/// Run one qif file at one config. Panics on any correctness failure; returns per-config
/// byte totals for the compression metric.
fn run_qif_at_config(qif_path: &Path, groups: &[qif::QifGroup], config: Config) -> EncodeStats {
    let encoder = EncoderDynamicTable::default();
    encoder.initialize_from_peer_settings(
        config.capacity as usize,
        H3Settings::default()
            .with_qpack_max_table_capacity(config.capacity)
            .with_qpack_blocked_streams(config.max_blocked),
    );
    let decoder = DecoderDynamicTable::new(config.capacity as usize, config.max_blocked as usize);

    // Drain the initial Set Dynamic Table Capacity instruction (if any) into the decoder so
    // that subsequent Insert instructions are accepted.
    let initial_ops: Vec<u8> = encoder.drain_pending_ops().into_iter().flatten().collect();
    let mut stats = EncodeStats::default();
    stats.encoder_stream_bytes += initial_ops.len();
    if !initial_ops.is_empty() {
        let mut cursor = Cursor::new(&initial_ops[..]);
        future::block_on(decoder.run_reader(&mut cursor)).unwrap_or_else(|e| {
            panic!(
                "{}: decoder rejected initial SetDynamicTableCapacity: {e}",
                qif_path.display()
            )
        });
    }

    for (i, group) in groups.iter().enumerate() {
        // Interop convention: stream ids start at 1 and increase monotonically.
        let stream_id = (i as u64) + 1;
        let field_lines = qif::build_field_lines(group)
            .unwrap_or_else(|e| panic!("{}: group {i}: {e}", qif_path.display()));

        let mut buf = Vec::new();
        encoder.encode_field_lines(&field_lines, &mut buf, stream_id);
        stats.section_bytes += buf.len();

        // Drain encoder-stream ops so the decoder has them when it decodes the header block.
        let enc_ops: Vec<u8> = encoder.drain_pending_ops().into_iter().flatten().collect();
        stats.encoder_stream_bytes += enc_ops.len();
        if !enc_ops.is_empty() {
            let mut cursor = Cursor::new(&enc_ops[..]);
            future::block_on(decoder.run_reader(&mut cursor)).unwrap_or_else(|e| {
                panic!(
                    "{}: group {i}: decoder run_reader failed: {e}",
                    qif_path.display()
                )
            });
        }

        let field_section = future::block_on(decoder.decode(&buf, stream_id))
            .unwrap_or_else(|e| panic!("{}: group {i}: decode failed: {e}", qif_path.display()));

        let mut got = qif::field_section_to_pairs(field_section);
        let mut want = group.clone();
        got.sort();
        want.sort();
        assert!(
            got == want,
            "{}: group {i} mismatch (section {} bytes, enc-stream {} bytes)\n  want: {:?}\n  got:  {:?}",
            qif_path.display(),
            buf.len(),
            enc_ops.len(),
            want,
            got,
        );

        // Always-ack: if the section emitted a non-zero prefix it registered an outstanding
        // section (see `emit_section_prefix` — section_ric=0 produces exactly `[0x00, 0x00]`,
        // any RIC>0 produces a non-zero first byte). Acknowledging advances KRC so the next
        // group can reference entries non-blockingly and pinned entries can be evicted.
        if !buf.starts_with(&[0x00, 0x00]) {
            encoder.on_section_ack(stream_id).unwrap_or_else(|e| {
                panic!(
                    "{}: group {i}: encoder rejected section ack: {e}",
                    qif_path.display()
                )
            });
        }
    }

    stats
}

// --- Compression metric ---

/// Sum the wire-bytes of every record in a reference `.out` file. Header-block record bytes
/// and encoder-stream record bytes are added together for an apples-to-apples comparison
/// with our [`EncodeStats::total`].
fn sum_reference_out_bytes(out_path: &Path) -> std::io::Result<usize> {
    let data = std::fs::read(out_path)?;
    let mut total = 0usize;
    let mut pos = 0usize;
    while pos + 12 <= data.len() {
        let length = u32::from_be_bytes(data[pos + 8..pos + 12].try_into().unwrap()) as usize;
        pos += 12;
        if pos + length > data.len() {
            break;
        }
        total += length;
        pos += length;
    }
    Ok(total)
}

/// Walk the reference-encoder corpus for `(stem, config)` and collect the totals per encoder.
///
/// Reference files are named `{stem}.out.{cap}.{blocked}.{variant}` where variant 1 =
/// immediate acks (the analogue of our always-ack substrate). We compare against variant 1
/// only, so the comparison is like-for-like.
fn reference_totals_for(encoded_dir: &Path, stem: &str, config: Config) -> BTreeMap<String, usize> {
    let want_suffix = format!(".out.{}.{}.1", config.capacity, config.max_blocked);
    let mut out = BTreeMap::new();
    let Ok(read_dir) = std::fs::read_dir(encoded_dir) else {
        return out;
    };
    for encoder_entry in read_dir.flatten() {
        let encoder_dir = encoder_entry.path();
        if !encoder_dir.is_dir() {
            continue;
        }
        let encoder_name = encoder_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        if encoder_name == "errors" || encoder_name == "examples" {
            continue;
        }
        let candidate = encoder_dir.join(format!("{stem}{want_suffix}"));
        if let Ok(total) = sum_reference_out_bytes(&candidate) {
            out.insert(encoder_name, total);
        }
    }
    out
}

// --- Test entry point ---

#[test]
fn qpack_encoder_corpus() {
    let _ = env_logger::try_init();
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/qifs");
    if !base.exists() {
        eprintln!("qifs submodule not checked out, skipping QPACK encoder corpus test");
        return;
    }

    let qif_dir = base.join("qifs");
    let encoded_dir = base.join("encoded/qpack-06");

    let filter = std::env::var("QPACK_ENCODER_CORPUS_FILTER").ok();
    let stats_enabled = std::env::var("QPACK_ENCODER_STATS").is_ok_and(|v| v == "1");

    // Collected (qif stem, config) -> (ours, per-encoder reference totals). Only populated
    // when stats are enabled.
    let mut metric: Vec<(String, Config, EncodeStats, BTreeMap<String, usize>)> = Vec::new();

    let mut tested = 0usize;

    let mut entries: Vec<_> = std::fs::read_dir(&qif_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", qif_dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "qif"))
        .collect();
    entries.sort();

    for qif_path in entries {
        if let Some(needle) = &filter
            && !qif_path.to_string_lossy().contains(needle.as_str())
        {
            continue;
        }

        let content = std::fs::read_to_string(&qif_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", qif_path.display()));
        let groups = qif::parse(&content);
        if groups.is_empty() {
            continue;
        }

        let stem = qif_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();

        for &config in CONFIGS {
            eprintln!(
                "testing {} @ ({}, {})",
                qif_path.display(),
                config.capacity,
                config.max_blocked
            );
            let stats = run_qif_at_config(&qif_path, &groups, config);
            tested += 1;

            if stats_enabled {
                let refs = reference_totals_for(&encoded_dir, &stem, config);
                metric.push((stem.clone(), config, stats, refs));
            }
        }
    }

    assert!(
        tested > 0,
        "no qif files were tested — check that tests/qifs is populated{}",
        filter
            .as_deref()
            .map(|f| format!(" or that QPACK_ENCODER_CORPUS_FILTER={f:?} matches something"))
            .unwrap_or_default()
    );

    if stats_enabled {
        print_metric_report(&metric);
    }
}

fn print_metric_report(metric: &[(String, Config, EncodeStats, BTreeMap<String, usize>)]) {
    eprintln!("\n=== QPACK Encoder Corpus — Compression Report ===");
    eprintln!(
        "(totals are section_bytes + encoder_stream_bytes; reference = variant 1 / \
         immediate-acks)\n"
    );
    eprintln!(
        "{:<20} {:<12} {:>10} {:>10} {:>10}  vs references (pct of ours)",
        "qif", "config", "section", "enc_stream", "total"
    );
    for (stem, config, stats, refs) in metric {
        let refs_str = if refs.is_empty() {
            String::from("(no reference at this config)")
        } else {
            refs.iter()
                .map(|(name, total)| {
                    let pct = if stats.total() > 0 {
                        (*total as f64) * 100.0 / (stats.total() as f64)
                    } else {
                        0.0
                    };
                    format!("{name}={total} ({pct:.1}%)")
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        eprintln!(
            "{:<20} {:<12} {:>10} {:>10} {:>10}  {}",
            stem,
            format!("({},{})", config.capacity, config.max_blocked),
            stats.section_bytes,
            stats.encoder_stream_bytes,
            stats.total(),
            refs_str,
        );
    }

    // Aggregate by config. Our total is summed across every qif (our baseline is unconditional).
    // Per-encoder comparisons are apples-to-apples: ours_matched sums our bytes only on files
    // where that encoder also has a reference at this config. This avoids penalizing encoders
    // that simply don't publish certain files (f5 has no .out.0.0.*, for instance).
    let mut ours_by_config: BTreeMap<(u64, u64), usize> = BTreeMap::new();
    // (cap, blocked, encoder) -> (our bytes on matching files, their bytes)
    let mut paired: BTreeMap<(u64, u64, String), (usize, usize)> = BTreeMap::new();
    for (_, config, stats, refs) in metric {
        *ours_by_config
            .entry((config.capacity, config.max_blocked))
            .or_default() += stats.total();
        for (name, total) in refs {
            let entry = paired
                .entry((config.capacity, config.max_blocked, name.clone()))
                .or_default();
            entry.0 += stats.total();
            entry.1 += *total;
        }
    }
    eprintln!(
        "\n--- aggregates across all qifs (per-encoder pct is apples-to-apples: only files where \
         that encoder has a reference) ---"
    );
    for ((cap, blk), ours) in &ours_by_config {
        let refs_str = paired
            .iter()
            .filter(|((c, b, _), _)| *c == *cap && *b == *blk)
            .map(|((_, _, name), (ours_matched, their_total))| {
                let pct = if *ours_matched > 0 {
                    (*their_total as f64) * 100.0 / (*ours_matched as f64)
                } else {
                    0.0
                };
                format!("{name}={their_total} ({pct:.1}% of ours on matching files)")
            })
            .collect::<Vec<_>>()
            .join("\n    ");
        eprintln!("({cap},{blk}): ours={ours}\n    {refs_str}");
    }
    eprintln!();
}
