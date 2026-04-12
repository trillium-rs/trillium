use super::{FieldSection, PseudoHeaders};
use crate::{Status, headers::qpack::decoder_dynamic_table::DecoderDynamicTable};
use futures_lite::{future, io::Cursor};
use std::{path::Path, sync::Arc, time::Duration};
use trillium_testing::RuntimeTrait as _;
use unicycle::FuturesUnordered;

/// Per-file decode timeout. A well-formed corpus file decodes every request in
/// microseconds; anything hitting this ceiling is a bug (likely a wake-plumbing
/// regression). One timeout per file (not per request) keeps the thread-per-task
/// default runtime from accumulating thousands of live delay threads across the
/// corpus walk.
const FILE_TIMEOUT: Duration = Duration::from_secs(5);

// --- QIF parsing ---

/// Parse a QIF file into an ordered list of header groups.
///
/// Each group is a `Vec<(name, value)>` of all headers (including pseudo-headers)
/// for one stream, in file order. Blank lines separate groups; `#`-prefixed lines
/// are comments. The name/value separator within a line is a tab character.
fn parse_qif(content: &str) -> Vec<Vec<(String, String)>> {
    let mut groups: Vec<Vec<(String, String)>> = Vec::new();
    let mut current: Vec<(String, String)> = Vec::new();

    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some((name, value)) = line.split_once('\t') {
            current.push((name.to_owned(), value.to_owned()));
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

// --- Binary interop format parsing ---

/// A single record from a QPACK offline-interop `.out` file.
///
/// `stream_id == 0` means encoder stream data; any other id is a header block.
struct OutRecord {
    stream_id: u64,
    data: Vec<u8>,
}

fn parse_out_records(data: &[u8]) -> Result<Vec<OutRecord>, String> {
    let mut records = Vec::new();
    let mut pos = 0;
    while pos + 12 <= data.len() {
        let stream_id = u64::from_be_bytes(data[pos..pos + 8].try_into().unwrap());
        let length = u32::from_be_bytes(data[pos + 8..pos + 12].try_into().unwrap()) as usize;
        pos += 12;
        if pos + length > data.len() {
            return Err(format!(
                "record at offset {} claims length {length} but only {} bytes remain",
                pos - 12,
                data.len() - pos
            ));
        }
        records.push(OutRecord {
            stream_id,
            data: data[pos..pos + length].to_vec(),
        });
        pos += length;
    }
    if pos != data.len() {
        return Err(format!(
            "trailing {} bytes after last record",
            data.len() - pos
        ));
    }
    Ok(records)
}

// --- FieldSection → comparable pairs ---

fn field_section_to_pairs(fs: FieldSection<'static>) -> Vec<(String, String)> {
    let (pseudos, headers) = fs.into_parts();
    let mut pairs = Vec::new();
    pseudo_pairs(&pseudos, &mut pairs);
    for (name, values) in headers.iter() {
        for value in values {
            // HTTP/3 §4.2: header field names MUST be lowercase. Normalize so that
            // static-table entries (stored in canonical HTTP/1.1 title-case) compare
            // correctly against QIF files (which use lowercase names).
            pairs.push((name.to_string().to_lowercase(), value.to_string()));
        }
    }
    pairs
}

fn pseudo_pairs(p: &PseudoHeaders<'_>, out: &mut Vec<(String, String)>) {
    if let Some(m) = p.method() {
        out.push((":method".into(), m.to_string()));
    }
    if let Some(s) = p.status() {
        // QIF uses the numeric status code only, e.g. "200".
        out.push((":status".into(), status_code_str(s)));
    }
    if let Some(v) = p.path() {
        out.push((":path".into(), v.to_string()));
    }
    if let Some(v) = p.scheme() {
        out.push((":scheme".into(), v.to_string()));
    }
    if let Some(v) = p.authority() {
        out.push((":authority".into(), v.to_string()));
    }
    if let Some(v) = p.protocol() {
        out.push((":protocol".into(), v.to_string()));
    }
}

/// Format a Status as its numeric code string ("200", "404", etc.) to match QIF.
fn status_code_str(s: Status) -> String {
    // Status Display may include the reason phrase; we want just the digits.
    s.to_string()
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .to_owned()
}

// --- Core runner ---

/// Run a single `.out` file against its corresponding `.qif` expected output.
///
/// Records in an interop `.out` file are in temporal order — encoder-stream records
/// (stream id 0) and header-block records are interleaved to reflect the order in
/// which bytes would have arrived on a real connection. Crucially, the encoder is
/// allowed to place a header-block record **before** the encoder-stream record that
/// carries the dynamic-table entries that block needs; a correct decoder must
/// suspend that header block until the entries arrive, then resume it.
///
/// We replay that temporal order on a single task:
///
/// * Encoder-stream records are processed inline via `process_encoder_stream`, which runs
///   synchronously over an in-memory cursor.
/// * Header-block records are pushed as futures into a `FuturesUnordered` and run concurrently with
///   the driver. A header block that references not-yet-inserted entries will suspend on
///   [`DecoderDynamicTable::get`], register an event-listener waker, and resume once a later
///   encoder-stream record inserts those entries.
///
/// After each record we opportunistically drain any now-ready futures, then drain
/// the rest after EOF. Every header block must complete — the corpus is a
/// self-contained encoding, so any block still blocked at EOF is a bug in our
/// decoder (not an expected "would block in a real deployment" outcome).
///
/// Each decode future is wrapped in a per-future timeout so a wake-machinery bug
/// surfaces as a clean failure pointing at a specific stream id rather than an
/// opaque hang.
fn run_interop_file(out_path: &Path, qif_path: &Path, capacity: usize) {
    let qif_content = std::fs::read_to_string(qif_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", qif_path.display()));
    let expected_groups = parse_qif(&qif_content);

    let out_data =
        std::fs::read(out_path).unwrap_or_else(|e| panic!("reading {}: {e}", out_path.display()));
    let records = parse_out_records(&out_data)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", out_path.display()));

    let header_count = records.iter().filter(|r| r.stream_id != 0).count();

    assert_eq!(
        header_count,
        expected_groups.len(),
        "{}: {} header blocks but {} QIF groups",
        out_path.display(),
        header_count,
        expected_groups.len()
    );

    if header_count == 0 {
        return;
    }

    let table = Arc::new(DecoderDynamicTable::new(capacity, usize::MAX));
    // Interop-format convention: the offline-interop wiki states that "for historical
    // reasons, the initial dynamic table capacity is the maximum dynamic table capacity."
    // This contradicts RFC 9204 (initial capacity is zero; encoder MUST send a Set
    // Dynamic Table Capacity instruction first), and encoders in this corpus rely on
    // the convention — they emit Insert instructions immediately without a preceding
    // capacity update. Compensate in the test harness rather than in the production
    // decoder. The Set Dynamic Table Capacity path is covered by unit tests elsewhere.
    if capacity > 0 {
        table
            .set_capacity(capacity)
            .expect("pre-setting capacity should always succeed at max_capacity");
    }
    let runtime = trillium_testing::runtime();

    // Completed results, keyed by (header_idx_in_qif, actual outcome). We key on
    // header index rather than stream id because a single .qif group is identified
    // by its position in the file, not by a stream id that could in principle be
    // reassigned. Stream id N corresponds to header index N-1 under the interop
    // convention that stream ids start at 1 and monotonically increase.
    type Outcome = Result<FieldSection<'static>, String>;
    let mut results: Vec<(usize, Outcome)> = Vec::with_capacity(header_count);

    let drive = async {
        let mut pending: FuturesUnordered<_> = FuturesUnordered::new();

        for record in &records {
            if record.stream_id == 0 {
                let mut cursor = Cursor::new(&record.data[..]);
                table.run_reader(&mut cursor).await.unwrap_or_else(|e| {
                    panic!("encoder stream error in {}: {e}", out_path.display())
                });
            } else {
                let table = Arc::clone(&table);
                let stream_id = record.stream_id;
                // Interop convention: stream ids begin at 1 and increase monotonically.
                let header_idx = (stream_id - 1) as usize;
                let data = record.data.clone();
                pending.push(async move {
                    let outcome: Outcome =
                        match FieldSection::decode_with_dynamic_table(&data, &table, stream_id)
                            .await
                        {
                            Ok(fs) => Ok(fs),
                            Err(e) => Err(format!("decode error: {e}")),
                        };
                    (header_idx, outcome)
                });
            }

            // Opportunistically collect any futures that became ready while we
            // were processing this record. `poll_once` polls the stream one time
            // without awaiting: `Some(Some(item))` = a ready item, `Some(None)` =
            // stream exhausted, `None` = nothing ready right now.
            loop {
                match future::poll_once(pending.next()).await {
                    Some(Some(item)) => results.push(item),
                    _ => break,
                }
            }
        }

        // Drain the rest. For a well-formed corpus + a correct decoder, everything
        // should already be ready by now — this is just the sequencing point that
        // makes sure we observed them.
        while let Some(item) = pending.next().await {
            results.push(item);
        }
    };

    runtime.block_on(async {
        if runtime.timeout(FILE_TIMEOUT, drive).await.is_none() {
            panic!(
                "{}: timed out after {FILE_TIMEOUT:?} — some stream is hung (likely a \
                 wake-plumbing bug in DecoderDynamicTable::get). Rerun with RUST_LOG=trace to \
                 narrow down which stream id never resolved.",
                out_path.display()
            );
        }
    });

    assert_eq!(
        results.len(),
        header_count,
        "{}: collected {} results but expected {header_count}",
        out_path.display(),
        results.len()
    );

    let mut failures: Vec<String> = Vec::new();
    for (header_idx, outcome) in results {
        let expected = &expected_groups[header_idx];
        match outcome {
            Err(e) => failures.push(format!("  header {header_idx}: {e}")),
            Ok(fs) => {
                let mut got: Vec<_> = field_section_to_pairs(fs);
                let mut want: Vec<_> = expected.clone();
                got.sort();
                want.sort();
                if got != want {
                    failures.push(format!(
                        "  header {header_idx}: mismatch\n    want: {want:?}\n    got:  {got:?}"
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{}:\n{}",
        out_path.display(),
        failures.join("\n")
    );
}

// --- Test entry point ---

#[test]
fn qpack_interop_corpus() {
    let _ = env_logger::try_init();
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/qifs");
    if !base.exists() {
        eprintln!("qifs submodule not checked out, skipping QPACK corpus test");
        return;
    }

    let encoded_dir = base.join("encoded/qpack-06");
    let qif_dir = base.join("qifs");

    // `QPACK_CORPUS_FILTER=substring` restricts the run to files whose path
    // contains that substring. Useful for narrowing to a single file or encoder
    // while debugging: `QPACK_CORPUS_FILTER=quinn/netbsd.out.0.0.0 cargo test ...`
    let filter = std::env::var("QPACK_CORPUS_FILTER").ok();

    let mut tested = 0usize;
    let mut skipped = 0usize;

    for encoder_entry in std::fs::read_dir(&encoded_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", encoded_dir.display()))
    {
        let encoder_dir = encoder_entry.unwrap().path();
        if !encoder_dir.is_dir() {
            continue; // skip loose files like draft-examples.out at the top level
        }
        let dir_name = encoder_dir.file_name().unwrap().to_string_lossy();
        if dir_name == "errors" {
            continue; // error cases are a separate concern
        }
        if dir_name == "examples" {
            // `examples/` contains a single illustrative file whose stem
            // (`examples`) does not correspond to any file in `qifs/`. It's
            // distinct from the encoder-vs-encoder corpus.
            continue;
        }

        for file_entry in std::fs::read_dir(&encoder_dir)
            .unwrap_or_else(|e| panic!("reading {}: {e}", encoder_dir.display()))
        {
            let out_path = file_entry.unwrap().path();
            let filename = out_path.file_name().unwrap().to_string_lossy();

            // Filename: {qif_stem}.out.{capacity}.{max_blocked}.{variant}
            let Some((qif_stem, config)) = filename.split_once(".out.") else {
                continue;
            };
            let config_parts: Vec<&str> = config.split('.').collect();
            if config_parts.len() < 3 {
                continue;
            }
            let Ok(capacity) = config_parts[0].parse::<usize>() else {
                continue;
            };

            if let Some(needle) = &filter {
                if !out_path.to_string_lossy().contains(needle.as_str()) {
                    continue;
                }
            }

            let qif_path = qif_dir.join(format!("{qif_stem}.qif"));
            if !qif_path.exists() {
                skipped += 1;
                continue;
            }

            eprintln!("testing {}", out_path.display());
            run_interop_file(&out_path, &qif_path, capacity);
            tested += 1;
        }
    }

    assert!(
        tested > 0,
        "no corpus files were tested — check that tests/qifs is populated{}",
        filter
            .as_deref()
            .map(|f| format!(" or that QPACK_CORPUS_FILTER={f:?} matches something"))
            .unwrap_or_default()
    );
    if skipped > 0 {
        eprintln!("QPACK corpus: {tested} files tested, {skipped} skipped (no matching .qif)");
    }
}
