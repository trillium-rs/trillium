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
//! The test simulates a "fully-acking" peer: it acks both axes of decoder→encoder
//! signaling that a healthy peer would emit.
//!
//! 1. **Section ack** (§4.4.1): after each successful decode, we call
//!    [`EncoderDynamicTable::on_section_ack`] on the stream if the encoded section had a non-zero
//!    Required Insert Count.
//! 2. **Insert count increment** (§4.4.3): after applying encoder-stream ops to the decoder, we
//!    advance the encoder's Known Received Count to its current `insert_count` via
//!    [`EncoderDynamicTable::on_insert_count_increment`]. Without this, warming inserts (which
//!    produce RIC=0 sections so the section-ack path doesn't fire) would pile up unreferenceable
//!    forever, and the metric would punish phase-2+ policies that insert without referencing in the
//!    same section.
//!
//! Together these are the upper bound on what a healthy peer would do — both
//! deterministic, so the compression metric is a stable baseline as policy changes are
//! made. A randomized-ack variant (fastrand-seeded, high-probability ack) is a reasonable
//! follow-up but must stay out of the compression-metric path.
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
//!
//! ## Chunking (phase-1 benchmark harness)
//!
//! `QPACK_CHUNK_SIZES=n1,n2,...` simulates "one .qif = many short connections" by resetting
//! the encoder + decoder every N groups. Values are comma-separated positive integers plus
//! the literal `inf` for "don't reset" (today's behavior). Example:
//! `QPACK_CHUNK_SIZES=1,5,20,100,inf`. When unset, the runner behaves exactly as before (one
//! connection per .qif, full reference-comparison report). When set, the reference-comparison
//! report is replaced by a curve report showing total bytes per (config, chunk size) — the
//! reference `.out` files are unchunked, so a like-for-like comparison at N<∞ would need
//! ls-qpack run through the same chunking harness (not yet implemented).

use super::{
    DecoderDynamicTable, EncoderDynamicTable, qif,
    reference_out::{
        OutGroup, WireHistogram, classify_encoder_stream, classify_header_block,
        histogram_from_out_file, parse_encoder_stream_for_dump, parse_header_block_for_dump,
        parse_out_groups, render_encoder_instruction, render_field_line,
    },
};
use crate::h3::H3Settings;
use futures_lite::{future, io::Cursor};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

// `Headers` iterates unknown names through a `HashMap`, so encoding from a `FieldSection`
// produces non-deterministic wire bytes when a group contains multiple unknown headers (the
// dynamic table gets populated in random order, which then ripples into every subsequent
// section's references). The corpus test bypasses `FieldSection` entirely and feeds qif-file
// order to [`EncoderDynamicTable::encode_field_lines`] so the compression metric is a stable
// baseline that policy changes can be measured against.

/// A `(max_capacity, max_blocked_streams)` pair. `u64` to match `H3Settings` units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Settings for the per-group ASCII dump. `None` disables dumping. When `Some`, each
/// group's ours-vs-theirs side-by-side is appended to the writer.
struct DumpCtx<'a> {
    writer: &'a mut BufWriter<File>,
    their_groups: &'a [OutGroup],
}

/// Run one qif file at one config. Panics on any correctness failure; returns per-config
/// byte totals for the compression metric, the strategy-counter snapshot, and a
/// wire-level histogram of our own output classified through the same parser we use on
/// reference `.out` files — for apples-to-apples comparison.
///
/// `chunk_size` = `None` is today's behavior (one encoder + decoder for the whole file, one
/// long connection). `Some(n)` simulates a fleet of n-group connections: the encoder and
/// decoder are torn down and reconstructed every n groups, and the returned stats sum
/// bytes across all chunks. Stream ids are 1-based within each chunk.
///
/// If `dump` is `Some`, a side-by-side instruction-level dump of ours-vs-ls-qpack is
/// appended to `dump.writer` for each group. In chunked mode, the reference `.out` file
/// that `dump.their_groups` came from is unchunked, so the their-side of the dump is
/// nonsensical past the first chunk boundary — dumping is intended for debugging the
/// unchunked case.
fn run_qif_at_config(
    qif_path: &Path,
    groups: &[qif::QifGroup],
    config: Config,
    chunk_size: Option<usize>,
    observer: std::sync::Arc<super::HeaderObserver>,
    mut dump: Option<DumpCtx<'_>>,
) -> (EncodeStats, WireHistogram) {
    let mut stats = EncodeStats::default();
    let mut wire = WireHistogram::default();
    if groups.is_empty() {
        return (stats, wire);
    }
    let effective_chunk_size = chunk_size.unwrap_or(groups.len()).max(1);

    let mut chunk_start = 0;
    while chunk_start < groups.len() {
        let chunk_end = (chunk_start + effective_chunk_size).min(groups.len());
        let chunk = &groups[chunk_start..chunk_end];

        // Build an HttpContext that holds this cell's shared observer (so cross-
        // connection priming works) and the cell's max table capacity (the encoder reads
        // it at construction).
        let mut context = crate::HttpContext::default();
        context.observer = observer.clone();
        context.config.h3_max_table_capacity = config.capacity as usize;
        let encoder = EncoderDynamicTable::new(&context);
        encoder.initialize_from_peer_settings(
            H3Settings::default()
                .with_qpack_max_table_capacity(config.capacity)
                .with_qpack_blocked_streams(config.max_blocked),
        );
        let decoder =
            DecoderDynamicTable::new(config.capacity as usize, config.max_blocked as usize);

        // Drain the initial Set Dynamic Table Capacity instruction (if any) into the decoder
        // so that subsequent Insert instructions are accepted.
        let initial_ops: Vec<u8> = encoder.drain_pending_ops().into_iter().flatten().collect();
        stats.encoder_stream_bytes += initial_ops.len();
        wire.encoder_stream_bytes += initial_ops.len() as u64;
        classify_encoder_stream(&initial_ops, &mut wire);
        if !initial_ops.is_empty() {
            let mut cursor = Cursor::new(&initial_ops[..]);
            future::block_on(decoder.run_reader(&mut cursor)).unwrap_or_else(|e| {
                panic!(
                    "{}: decoder rejected initial SetDynamicTableCapacity: {e}",
                    qif_path.display()
                )
            });
        }

        for (i, group) in chunk.iter().enumerate() {
            // Interop convention: stream ids start at 1 and increase monotonically within
            // a connection. In chunked mode each chunk is a fresh connection, so ids restart.
            let stream_id = (i as u64) + 1;
            let global_index = chunk_start + i;
            let field_lines = qif::build_field_lines(group)
                .unwrap_or_else(|e| panic!("{}: group {global_index}: {e}", qif_path.display()));

            let mut buf = Vec::new();
            encoder.encode_field_lines(&field_lines, &mut buf, stream_id);
            stats.section_bytes += buf.len();
            wire.section_bytes += buf.len() as u64;
            wire.n_sections += 1;
            classify_header_block(&buf, &mut wire);

            // Drain encoder-stream ops so the decoder has them when it decodes the header
            // block.
            let enc_ops: Vec<u8> = encoder.drain_pending_ops().into_iter().flatten().collect();
            stats.encoder_stream_bytes += enc_ops.len();
            wire.encoder_stream_bytes += enc_ops.len() as u64;
            classify_encoder_stream(&enc_ops, &mut wire);

            // Per-group ASCII dump. Appended to the writer if `dump` was provided — compares
            // our emission sequence to the reference encoder's for this same group.
            if let Some(ctx) = dump.as_mut() {
                let their_group = ctx.their_groups.get(global_index);
                let our_snapshot = OurStateSnapshot {
                    insert_count: encoder.insert_count(),
                    entry_count: encoder.entry_count(),
                    current_size: encoder.current_size(),
                    capacity: encoder.capacity(),
                };
                dump_group(
                    ctx.writer,
                    global_index,
                    stream_id,
                    group,
                    &enc_ops,
                    &buf,
                    their_group,
                    &our_snapshot,
                );
            }
            if !enc_ops.is_empty() {
                let mut cursor = Cursor::new(&enc_ops[..]);
                future::block_on(decoder.run_reader(&mut cursor)).unwrap_or_else(|e| {
                    panic!(
                        "{}: group {global_index}: decoder run_reader failed: {e}",
                        qif_path.display()
                    )
                });

                // Mimic the §4.4.3 Insert Count Increment a real peer's decoder would send
                // after processing those Inserts — without it, KRC stays stuck at 0 for any
                // section whose RIC is also 0 (the warming-insert pattern), starving every
                // subsequent encode of references to entries the decoder already has.
                let increment = encoder.insert_count() - encoder.known_received_count();
                if increment > 0 {
                    encoder
                        .on_insert_count_increment(increment)
                        .unwrap_or_else(|e| {
                            panic!(
                                "{}: group {global_index}: encoder rejected insert count \
                                 increment {increment}: {e}",
                                qif_path.display()
                            )
                        });
                }
            }

            let field_section =
                future::block_on(decoder.decode(&buf, stream_id)).unwrap_or_else(|e| {
                    panic!(
                        "{}: group {global_index}: decode failed: {e}",
                        qif_path.display()
                    )
                });

            let mut got = qif::field_section_to_pairs(field_section);
            let mut want = group.clone();
            got.sort();
            want.sort();
            assert!(
                got == want,
                "{}: group {global_index} mismatch (section {} bytes, enc-stream {} bytes)\n  \
                 want: {:?}\n  got:  {:?}",
                qif_path.display(),
                buf.len(),
                enc_ops.len(),
                want,
                got,
            );

            // Always-ack: if the section emitted a non-zero prefix it registered an
            // outstanding section (see `emit_section_prefix` — section_ric=0 produces
            // exactly `[0x00, 0x00]`, any RIC>0 produces a non-zero first byte).
            // Acknowledging advances KRC so the next group can reference entries
            // non-blockingly and pinned entries can be evicted.
            if !buf.starts_with(&[0x00, 0x00]) {
                encoder.on_section_ack(stream_id).unwrap_or_else(|e| {
                    panic!(
                        "{}: group {global_index}: encoder rejected section ack: {e}",
                        qif_path.display()
                    )
                });
            }
        }

        chunk_start = chunk_end;
    }

    (stats, wire)
}

/// Post-section state snapshot on our side. Captured after encoding the section and
/// included in the per-group dump so we can compare fill levels to what ls-qpack is doing
/// in parallel.
struct OurStateSnapshot {
    insert_count: u64,
    entry_count: usize,
    current_size: usize,
    capacity: usize,
}

/// Write one group's ours-vs-theirs dump to `writer`. Classifies both sides' raw bytes
/// into a compact one-line-per-instruction format suitable for terminal diffing or for
/// feeding to an LLM session hunting for patterns.
fn dump_group(
    writer: &mut BufWriter<File>,
    group_index: usize,
    stream_id: u64,
    input: &qif::QifGroup,
    our_enc: &[u8],
    our_hdr: &[u8],
    their_group: Option<&OutGroup>,
    our_snapshot: &OurStateSnapshot,
) {
    let _ = writeln!(
        writer,
        "==== group {group_index} (stream_id={stream_id}) ===="
    );
    let fill_pct = if our_snapshot.capacity == 0 {
        0.0
    } else {
        (our_snapshot.current_size as f64) * 100.0 / (our_snapshot.capacity as f64)
    };
    let _ = writeln!(
        writer,
        "  our_state_after: insert_count={} entry_count={} used={}/{} ({:.1}%)",
        our_snapshot.insert_count,
        our_snapshot.entry_count,
        our_snapshot.current_size,
        our_snapshot.capacity,
        fill_pct,
    );
    let _ = writeln!(writer, "  input:");
    for (name, value) in input {
        let _ = writeln!(
            writer,
            "    {name}: {}",
            render_bytes_for_dump(value.as_bytes())
        );
    }

    let _ = writeln!(writer, "  ours:");
    dump_enc_and_hdr(writer, our_enc, our_hdr);

    let _ = writeln!(writer, "  ls-qpack:");
    if let Some(g) = their_group {
        for chunk in &g.enc_stream {
            for instr in parse_encoder_stream_for_dump(chunk) {
                let _ = writeln!(writer, "    enc: {}", render_encoder_instruction(&instr));
            }
        }
        if let Some((prefix, lines)) = parse_header_block_for_dump(&g.header_block) {
            let _ = writeln!(
                writer,
                "    hdr.prefix: enc_ric={} sign={} delta_base={}",
                prefix.encoded_required_insert_count,
                u8::from(prefix.base_is_negative),
                prefix.delta_base,
            );
            for instr in lines {
                let _ = writeln!(writer, "    hdr: {}", render_field_line(&instr));
            }
        }
    } else {
        let _ = writeln!(writer, "    (no reference data for this group)");
    }
    let _ = writeln!(writer);
}

fn dump_enc_and_hdr(writer: &mut BufWriter<File>, enc: &[u8], hdr: &[u8]) {
    for instr in parse_encoder_stream_for_dump(enc) {
        let _ = writeln!(writer, "    enc: {}", render_encoder_instruction(&instr));
    }
    if let Some((prefix, lines)) = parse_header_block_for_dump(hdr) {
        let _ = writeln!(
            writer,
            "    hdr.prefix: enc_ric={} sign={} delta_base={}",
            prefix.encoded_required_insert_count,
            u8::from(prefix.base_is_negative),
            prefix.delta_base,
        );
        for instr in lines {
            let _ = writeln!(writer, "    hdr: {}", render_field_line(&instr));
        }
    }
}

/// Mirror of `reference_out::render_value` for raw bytes coming from qif groups. Kept
/// local so the qif value rendering stays in test-module scope.
fn render_bytes_for_dump(bytes: &[u8]) -> String {
    const LIMIT: usize = 80;
    let mut out = String::with_capacity(bytes.len().min(LIMIT) + 2);
    out.push('"');
    for (i, &b) in bytes.iter().enumerate() {
        if i >= LIMIT {
            out.push_str("...");
            break;
        }
        match b {
            0x20..=0x7e if b != b'"' && b != b'\\' => out.push(b as char),
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
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
/// only, so the comparison is like-for-like. Returns `(byte total, wire histogram)` per
/// reference encoder — the histogram is `None` when the parser couldn't walk the file.
fn reference_stats_for(
    encoded_dir: &Path,
    stem: &str,
    config: Config,
) -> BTreeMap<String, (usize, Option<WireHistogram>)> {
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
            let hist = histogram_from_out_file(&candidate).ok().flatten();
            out.insert(encoder_name, (total, hist));
        }
    }
    out
}

// --- Test entry point ---

/// Parse `QPACK_CHUNK_SIZES`. Returns `vec![None]` when unset (today's behavior — one
/// connection per qif). Comma-separated integers become `Some(n)`; `inf` (or empty token)
/// becomes `None`. Panics on a malformed integer so a typo fails the test loudly.
fn parse_chunk_sizes() -> Vec<Option<usize>> {
    match std::env::var("QPACK_CHUNK_SIZES").ok() {
        None => vec![None],
        Some(s) => s
            .split(',')
            .map(str::trim)
            .filter(|tok| !tok.is_empty())
            .map(|tok| match tok {
                "inf" | "∞" => None,
                n => Some(
                    n.parse::<usize>()
                        .unwrap_or_else(|_| panic!("QPACK_CHUNK_SIZES: invalid token {n:?}")),
                ),
            })
            .collect(),
    }
}

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
    let chunk_sizes = parse_chunk_sizes();
    let chunked_mode = chunk_sizes.iter().any(Option::is_some);
    // When set, dump per-group ours-vs-ls-qpack ASCII to `target/qpack-dump/`. Value is a
    // substring filter over qif stems — e.g. `QPACK_ENCODER_DUMP=fb-resp` writes dumps
    // only for fb-resp / fb-resp-hq at each config. Scaffolding; tears out. Only fires at
    // `chunk_size = None` (the reference `.out` files are unchunked, so any chunked
    // their-side would be misaligned past the first chunk boundary).
    let dump_filter = std::env::var("QPACK_ENCODER_DUMP").ok();
    let dump_dir: Option<PathBuf> = dump_filter.as_ref().map(|_| {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/qpack-dump");
        let _ = std::fs::create_dir_all(&d);
        d
    });

    // Collected (chunk_size, qif stem, config) -> (ours, per-encoder reference totals +
    // histograms, our wire histogram). Only populated when stats are enabled. `chunk_size
    // = None` means one connection per qif (unchunked); `Some(n)` means the qif was
    // replayed as many n-group connections.
    type MetricRow = (
        Option<usize>,
        String,
        Config,
        EncodeStats,
        BTreeMap<String, (usize, Option<WireHistogram>)>,
        WireHistogram,
    );
    let mut metric: Vec<MetricRow> = Vec::new();

    // One header observer per (chunk_size, config) cell — accumulates across all qifs at
    // that config, matching the "single listener sees all this traffic" model and giving
    // the observer enough ticks to pass its warmup threshold. Lazily populated.
    let mut observers: std::collections::HashMap<
        (Option<usize>, Config),
        std::sync::Arc<super::HeaderObserver>,
    > = std::collections::HashMap::new();

    let mut tested = 0usize;

    let mut entries: Vec<_> = std::fs::read_dir(&qif_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", qif_dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "qif"))
        // `draft-examples.qif` is the RFC 9204 Appendix B illustrative file. None of the
        // reference encoders publish outputs for it at the configs we test, so it has
        // nothing to contribute to the compression comparison and would only show up as
        // an unmatched-file artifact in the aggregate report.
        .filter(|p| p.file_stem().is_none_or(|s| s != "draft-examples"))
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

        for &chunk_size in &chunk_sizes {
            for &config in CONFIGS {
                eprintln!(
                    "testing {} @ ({}, {}) chunk={}",
                    qif_path.display(),
                    config.capacity,
                    config.max_blocked,
                    chunk_size.map_or_else(|| "inf".to_string(), |n| n.to_string()),
                );
                // Dump writer only opens when unchunked — avoids a meaningless
                // their-side diff in chunked runs.
                let dump_file = if chunk_size.is_none() {
                    dump_filter.as_deref().and_then(|needle| {
                        if !stem.contains(needle) {
                            return None;
                        }
                        let path = dump_dir.as_ref()?.join(format!(
                            "{stem}_{}_{}.txt",
                            config.capacity, config.max_blocked
                        ));
                        let file = File::create(&path).ok()?;
                        Some((path, BufWriter::new(file)))
                    })
                } else {
                    None
                };
                let (their_groups, mut dump_writer) = match dump_file {
                    Some((path, mut w)) => {
                        // Preload ls-qpack's .out so we can diff per group.
                        let reference_path = encoded_dir.join(format!(
                            "ls-qpack/{stem}.out.{}.{}.1",
                            config.capacity, config.max_blocked
                        ));
                        let groups = parse_out_groups(&reference_path).unwrap_or_default();
                        let _ = writeln!(
                            &mut w,
                            "# {} @ ({},{}) — ours vs ls-qpack, one line per instruction",
                            qif_path.display(),
                            config.capacity,
                            config.max_blocked
                        );
                        let _ = writeln!(&mut w, "# reference: {}", reference_path.display());
                        let _ = writeln!(&mut w);
                        eprintln!("    (dumping to {})", path.display());
                        (Some(groups), Some(w))
                    }
                    None => (None, None),
                };
                let dump_ctx = match (their_groups.as_deref(), dump_writer.as_mut()) {
                    (Some(g), Some(w)) => Some(DumpCtx {
                        writer: w,
                        their_groups: g,
                    }),
                    _ => None,
                };
                let observer = observers
                    .entry((chunk_size, config))
                    .or_insert_with(|| {
                        std::sync::Arc::new(super::HeaderObserver::from_http_config(
                            &crate::HttpConfig::DEFAULT,
                        ))
                    })
                    .clone();
                let (stats, our_wire) =
                    run_qif_at_config(&qif_path, &groups, config, chunk_size, observer, dump_ctx);
                tested += 1;

                if stats_enabled {
                    // Reference files are unchunked, so per-encoder refs are only
                    // meaningful when chunk_size is None. Skip the lookup for chunked
                    // rows — they render through the curve report which ignores refs.
                    let refs = if chunk_size.is_none() {
                        reference_stats_for(&encoded_dir, &stem, config)
                    } else {
                        BTreeMap::new()
                    };
                    metric.push((chunk_size, stem.clone(), config, stats, refs, our_wire));
                }
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
        if chunked_mode {
            print_curve_report(&metric, &chunk_sizes);
        } else {
            // Unchunked rows still flow through the legacy report verbatim.
            let unchunked: Vec<_> = metric
                .iter()
                .filter(|row| row.0.is_none())
                .map(|(_, stem, c, s, r, w)| (stem.clone(), *c, *s, r.clone(), *w))
                .collect();
            print_metric_report(&unchunked);
        }
    }
}

/// Curve report: total encoded bytes per `(qif, config, chunk_size)` plus an aggregate
/// across qifs. Rendered when `QPACK_CHUNK_SIZES` is set (replaces the reference-comparison
/// report, which would be meaningless at `chunk_size < ∞` since reference `.out` files are
/// unchunked). Chunk sizes are rendered in the order the user supplied them so the curve
/// reads left-to-right in whatever direction is intuitive for the sweep being run.
fn print_curve_report(
    metric: &[(
        Option<usize>,
        String,
        Config,
        EncodeStats,
        BTreeMap<String, (usize, Option<WireHistogram>)>,
        WireHistogram,
    )],
    chunk_sizes: &[Option<usize>],
) {
    eprintln!("\n=== QPACK Encoder Curve — total bytes by chunk size ===");
    eprintln!(
        "(bytes = section_bytes + encoder_stream_bytes; chunk_size = simulated connection length \
         in header blocks; inf = one connection per qif)\n"
    );

    let fmt_cs =
        |cs: Option<usize>| -> String { cs.map_or_else(|| "N=inf".into(), |n| format!("N={n}")) };

    // Build: (stem, cap, blk) -> chunk_size -> total bytes.
    let mut per_qif: BTreeMap<(String, u64, u64), BTreeMap<Option<usize>, usize>> = BTreeMap::new();
    let mut agg: BTreeMap<(u64, u64), BTreeMap<Option<usize>, usize>> = BTreeMap::new();
    for (cs, stem, config, stats, _, _) in metric {
        *per_qif
            .entry((stem.clone(), config.capacity, config.max_blocked))
            .or_default()
            .entry(*cs)
            .or_default() += stats.total();
        *agg.entry((config.capacity, config.max_blocked))
            .or_default()
            .entry(*cs)
            .or_default() += stats.total();
    }

    // Header row, per-qif.
    let mut header = format!("{:<20} {:<12}", "qif", "config");
    for &cs in chunk_sizes {
        header.push_str(&format!(" {:>12}", fmt_cs(cs)));
    }
    eprintln!("{header}");
    for ((stem, cap, blk), by_chunk) in &per_qif {
        let config_str = format!("({cap},{blk})");
        let mut row = format!("{stem:<20} {config_str:<12}");
        for &cs in chunk_sizes {
            let val = by_chunk.get(&cs).copied().unwrap_or(0);
            row.push_str(&format!(" {val:>12}"));
        }
        eprintln!("{row}");
    }

    eprintln!("\n--- aggregate across qifs ---");
    let mut header = format!("{:<12}", "config");
    for &cs in chunk_sizes {
        header.push_str(&format!(" {:>12}", fmt_cs(cs)));
    }
    eprintln!("{header}");
    for ((cap, blk), by_chunk) in &agg {
        let config_str = format!("({cap},{blk})");
        let mut row = format!("{config_str:<12}");
        for &cs in chunk_sizes {
            let val = by_chunk.get(&cs).copied().unwrap_or(0);
            row.push_str(&format!(" {val:>12}"));
        }
        eprintln!("{row}");
    }

    // Ratio view: bytes at each chunk size as a % of the rightmost-available chunk size
    // (typically N=inf if supplied). Makes "how much does chunking cost us?" legible at a
    // glance without mental arithmetic.
    let baseline = chunk_sizes
        .iter()
        .rev()
        .copied()
        .find(|cs| agg.values().any(|m| m.contains_key(cs)));
    if let Some(base_cs) = baseline {
        eprintln!(
            "\n--- aggregate ratio to {} (lower is better; >100% = chunking costs bytes) ---",
            fmt_cs(base_cs)
        );
        let mut header = format!("{:<12}", "config");
        for &cs in chunk_sizes {
            header.push_str(&format!(" {:>10}", fmt_cs(cs)));
        }
        eprintln!("{header}");
        for ((cap, blk), by_chunk) in &agg {
            let config_str = format!("({cap},{blk})");
            let mut row = format!("{config_str:<12}");
            let base = by_chunk.get(&base_cs).copied().unwrap_or(0);
            for &cs in chunk_sizes {
                let val = by_chunk.get(&cs).copied().unwrap_or(0);
                let cell = if base == 0 {
                    "  —".to_string()
                } else {
                    format!("{:.1}%", (val as f64) * 100.0 / (base as f64))
                };
                row.push_str(&format!(" {cell:>10}"));
            }
            eprintln!("{row}");
        }
    }
    eprintln!();
}

fn print_metric_report(
    metric: &[(
        String,
        Config,
        EncodeStats,
        BTreeMap<String, (usize, Option<WireHistogram>)>,
        WireHistogram,
    )],
) {
    eprintln!("\n=== QPACK Encoder Corpus — Compression Report ===");
    eprintln!(
        "(totals are section_bytes + encoder_stream_bytes; reference = variant 1 / \
         immediate-acks)\n"
    );
    eprintln!(
        "{:<20} {:<12} {:>10} {:>10} {:>10}  vs references (pct of ours)",
        "qif", "config", "section", "enc_stream", "total"
    );
    for (stem, config, stats, refs, _) in metric {
        let refs_str = if refs.is_empty() {
            String::from("(no reference at this config)")
        } else {
            refs.iter()
                .map(|(name, (total, _))| {
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
    // (cap, blocked) -> our wire histogram summed across all qifs
    let mut our_wire_by_config: BTreeMap<(u64, u64), WireHistogram> = BTreeMap::new();
    // (cap, blocked, encoder) -> their wire histogram summed across qifs where they
    // have a reference at this config
    let mut their_wire_by_config: BTreeMap<(u64, u64, String), WireHistogram> = BTreeMap::new();
    for (_, config, stats, refs, our_wire) in metric {
        *ours_by_config
            .entry((config.capacity, config.max_blocked))
            .or_default() += stats.total();
        for (name, (total, hist)) in refs {
            let entry = paired
                .entry((config.capacity, config.max_blocked, name.clone()))
                .or_default();
            entry.0 += stats.total();
            entry.1 += *total;
            if let Some(h) = hist {
                their_wire_by_config
                    .entry((config.capacity, config.max_blocked, name.clone()))
                    .or_default()
                    .add(h);
            }
        }
        our_wire_by_config
            .entry((config.capacity, config.max_blocked))
            .or_default()
            .add(our_wire);
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
    print_wire_comparison(&our_wire_by_config, &their_wire_by_config);
}

/// Side-by-side wire-instruction histogram comparison, per `(cap, blocked)` config. Reads
/// ours vs each reference encoder and flags buckets that diverge materially.
///
/// "Indexed dynamic total" and "literal dyn name total" collapse pre-base / post-base
/// variants that alias across encoders — we use pre-base with `delta_base=0`, ls-qpack
/// uses post-base. Separating them in the raw rows would exaggerate divergence.
fn print_wire_comparison(
    ours: &BTreeMap<(u64, u64), WireHistogram>,
    theirs: &BTreeMap<(u64, u64, String), WireHistogram>,
) {
    eprintln!("--- wire-instruction histograms (ours vs references) ---");
    for ((cap, blk), our) in ours {
        let mut refs: Vec<(&String, &WireHistogram)> = theirs
            .iter()
            .filter(|((c, b, _), _)| *c == *cap && *b == *blk)
            .map(|((_, _, name), h)| (name, h))
            .collect();
        refs.sort_by_key(|(name, _)| name.as_str());

        eprintln!(
            "\n({cap},{blk})  — bucket: ours vs [{}]",
            refs.iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );

        print_row("set_capacity", our.set_capacity, &refs, |h| h.set_capacity);
        print_row("insert_literal_name", our.insert_literal_name, &refs, |h| {
            h.insert_literal_name
        });
        print_row(
            "insert_static_name_ref",
            our.insert_static_name_ref,
            &refs,
            |h| h.insert_static_name_ref,
        );
        print_row(
            "insert_dynamic_name_ref",
            our.insert_dynamic_name_ref,
            &refs,
            |h| h.insert_dynamic_name_ref,
        );
        print_row("duplicate", our.duplicate, &refs, |h| h.duplicate);
        print_row(
            "  ∑ inserts (non-dup)",
            our.inserts_total(),
            &refs,
            WireHistogram::inserts_total,
        );

        eprintln!("  --");
        print_row("indexed_static", our.indexed_static, &refs, |h| {
            h.indexed_static
        });
        print_row(
            "indexed_dyn_pre_base",
            our.indexed_dynamic_pre_base,
            &refs,
            |h| h.indexed_dynamic_pre_base,
        );
        print_row("indexed_post_base", our.indexed_post_base, &refs, |h| {
            h.indexed_post_base
        });
        print_row(
            "  ∑ indexed dynamic",
            our.indexed_dynamic_total(),
            &refs,
            WireHistogram::indexed_dynamic_total,
        );
        print_row(
            "literal_static_name_ref",
            our.literal_static_name_ref,
            &refs,
            |h| h.literal_static_name_ref,
        );
        print_row(
            "literal_dyn_name_ref",
            our.literal_dynamic_name_ref,
            &refs,
            |h| h.literal_dynamic_name_ref,
        );
        print_row(
            "literal_post_base_name_ref",
            our.literal_post_base_name_ref,
            &refs,
            |h| h.literal_post_base_name_ref,
        );
        print_row(
            "  ∑ literal dyn name",
            our.literal_dyn_name_total(),
            &refs,
            WireHistogram::literal_dyn_name_total,
        );
        print_row(
            "literal_literal_name",
            our.literal_literal_name,
            &refs,
            |h| h.literal_literal_name,
        );
    }
    eprintln!();
}

/// One bucket row in the comparison. Prints our value, then each reference value with
/// a `Δ=Nx` marker when the reference differs from ours by ≥ 2× (in either direction).
fn print_row(
    label: &str,
    ours: u64,
    refs: &[(&String, &WireHistogram)],
    accessor: impl Fn(&WireHistogram) -> u64,
) {
    let refs_str = refs
        .iter()
        .map(|(name, h)| {
            let v = accessor(h);
            let flag = diverges(ours, v);
            format!("{name}={v}{flag}")
        })
        .collect::<Vec<_>>()
        .join("  ");
    eprintln!("  {label:<28} ours={ours:<8} {refs_str}");
}

fn diverges(ours: u64, theirs: u64) -> &'static str {
    if ours == 0 && theirs == 0 {
        return "";
    }
    if ours == 0 || theirs == 0 {
        return "  ←Δ";
    }
    let (a, b) = if ours > theirs {
        (ours, theirs)
    } else {
        (theirs, ours)
    };
    if a >= b * 2 { "  ←Δ" } else { "" }
}
