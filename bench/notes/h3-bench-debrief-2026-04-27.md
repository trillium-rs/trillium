# trillium h3 bench debrief — 2026-04-27 (afternoon)

Continuation of the morning's h2 session. Same instance (c7i.2xlarge AWS, 4 cores 0–3 server, 4 cores 4–7 load, loopback, jemalloc, aws-lc-rs, NODELAY enabled on TCP). Today's focus: add HTTP/3 to the bench harness, build a comparative table against hyper, profile the gaps.

## Setup additions

1. **`h2load` with HTTP/3** — Ubuntu's stock `h2load` (1.59.0 from apt) is built without ngtcp2/nghttp3 and can't speak h3. Built from source under `~/build/h3-h2load/` following [nghttp2's package README](https://nghttp2.org/documentation/package_README.html#build-http-3-enabled-h2load-and-nghttpx): aws-lc v1.72.0 → nghttp3 v1.15.0 → ngtcp2 v1.22.1 (with `--with-boringssl` against aws-lc) → nghttp2 master with `--enable-http3`. Required `g++-14` + `libstdc++-14-dev` to satisfy the C++23 `std::expected`/`std::concepts` requirements; `clang-18` advertises `__cpp_concepts=201907L` while libstdc++ gates `std::expected` on `>= 202002L`. The full chain is ~30 minutes including downloads. Resulting `h2load --h3 https://.../tiny` works.

2. **Trillium bench server** — added `.with_quic(QuicConfig::from_single_cert(&cert, &key))` to `bench/src/bin/trillium-server.rs` (one line). Also expanded the CLI surface from 8 hand-picked args to one `Option<T>` per `HttpConfig` field, so any tunable can be swept without recompiling. Added `--no-quic` to fall back to TCP-only for control runs.

3. **Hyper baseline** — added an h3 path via the `h3` crate (hyper-team-maintained) on top of `h3-quinn`. Hyper itself doesn't ship h3, so this is the natural h3 stack a hyper user reaches for today. UDP listener runs alongside the existing TCP listener on the same port; same routes, same TLS cert. Found and worked around an h3-quinn quirk (next section).

4. **`bench/scripts/h3-sweep.sh`** — paralleling `baseline.sh`. Runs the same 10-endpoint table as the h1/h2 sweeps with `--h3 -c 8 -m 10` (8 connections × 10 streams).

## Bug found: h3-quinn `RequestStream` `Drop` issues STOP_SENDING(0)

When `h3::server::RequestStream` is dropped without the recv side reaching FIN, h3-quinn issues `STOP_SENDING(0x0)` on the request stream. h2load reads any STOP_SENDING with a non-`H3_NO_ERROR` (`0x100`) code as a stream reset and counts the stream as errored — even though the response data was already delivered with FIN. Initial benchmark showed hyper h3 at "3,100 req/s" because of this — actual response 2xx count was 1.1M/s, h2load just refused to count any of it as "succeeded".

Fix in `bench/src/bin/hyper-server.rs`: explicitly `stream.stop_sending(Code::H3_NO_ERROR)` before drop. After that hyper h3 reports 218k req/s with 1.09M/1.09M succeeded — which matches the actual response throughput.

Worth filing upstream that h3-quinn's default Drop behavior should probably be `STOP_SENDING(H3_NO_ERROR)` rather than `STOP_SENDING(0)`. The h3 canonical example doesn't drain either, so anyone benchmarking through h2load hits this.

## Fix shipped this session: hashbrown swap in h3 QPACK encoder

Same shape as the morning's h2 driver fix. Three files swapped from `std::collections::HashMap` to `hashbrown::HashMap` (which uses foldhash by default):

- `http/src/headers/qpack/encoder_dynamic_table/state.rs` — `outstanding_sections`, `by_name`, `by_value`
- `http/src/headers/qpack/header_observer.rs` — `entries`
- `http/src/headers/qpack/encoder_dynamic_table/connection_metrics.rs` — `primed_entries`

Profile showed `Sip13Rounds::write` at 1.16% before the swap, gone after. Net throughput improvement: **~1–2% across all endpoints**. Smaller than the h2 driver's win because the SipHash hits per-encode rather than per-frame (h2 hit it on every header line read). Tests pass except for two pre-existing failures (qif corpus submodule data not present locally — unrelated, also fail on unmodified branch).

Worth correcting an earlier false claim: I initially measured "+12.6% on /tiny" from this swap based on a single ad-hoc benchmark (148k post-fix). Re-running inside the controlled sweep gave 133k, matching baseline + 1.2%. The 148k spike was run-to-run variance combined with me pattern-matching to an expected delta. The honest number is 1–2%.

## Final headline numbers (h3, post-hashbrown)

h2load `--h3 -c 8 -m 10 --duration=8 --warm-up-time=1`. Both servers on jemalloc, NODELAY-on-TCP (irrelevant for h3 but kept for cross-protocol consistency), default HttpConfig.

| endpoint | trillium r/s | hyper r/s | t/h | story |
|---|---:|---:|---:|---|
| /tiny       | 133,209 | 222,452 | 0.60x | hyper wins on small responses (per-request overhead) |
| /1k         | 128,581 | 206,553 | 0.62x | same shape as /tiny |
| /16k        |  79,775 |  98,754 | 0.81x | per-byte cost dominates |
| /1m         |   1,926 |   2,193 | 0.88x | bandwidth-bound, near-tied |
| /echo64     | 132,691 |  84,447 | **1.57x** | trillium wins; hyper at 247% CPU not saturated |
| /echo16k    |  42,340 |  48,017 | 0.88x | close |
| /echo1m     |     493 |     825 | 0.60x | hyper wins bulk upload-and-reflect |
| /recv64     | 133,791 |  84,079 | **1.59x** | trillium wins; hyper at 240% CPU not saturated |
| /recv16k    |  61,937 |  83,702 | 0.74x | hyper wins |
| /recv1m     |     721 |   2,006 | **0.36x** | hyper wins big on bulk upload (recv-path issue) |

## Profile decomposition (`/tiny`)

`perf record -F 999 -g --call-graph=dwarf,16384 -p PID -- sleep 6` against a steady-state run. Both servers spend ~32% of CPU in shared `quinn_proto` paths (`process_payload` 21.8%, `VarInt::decode` 5.3%, `frame::Iter::next` 5.0%) — the QUIC protocol floor, neither stack's fault. Note that `quinn_proto::frame::Iter` is the **QUIC** layer, not HTTP/3; trillium's own `h3::frame` parser shows up at 0.06% combined on /tiny (tiny because each request has only 3 H3 frame headers — 1 in, 2 out — at sub-µs each).

Trillium-specific costs on top of the QUIC floor (post-hashbrown):

| % | symbol | category |
|---:|---|---|
| 3.71 | `qpack::encoder_dynamic_table::encode_field_lines` | **observer bookkeeping inside the encode hot path** |
| 3.01 | `H3Connection::process_inbound_bidi` | async-fn state machine for the request lifecycle (orchestration overhead) |
| 2.25 | `encode_field_section_h3` body | Vec churn (alloc + resize + extend_from_slice memcpy per response) |
| 1.16 | `TokioRuntime::spawn` | spawn site #1 |
| 0.86 | `Runtime::spawn` (run_h3_connection) | spawn site #2 |
| 0.77 | `ConnectionRef::clone` | Arc clone in hot path |
| 0.75 | `Runtime::spawn` (handle_bidi_stream) | spawn site #3 |
| 0.57 | `spawn_inbound_uni_streams` | spawn site #4 |

Sum: ~13% of CPU directly attributable to trillium-side overhead. Hyper-side equivalent costs sum to ~9%. Difference = roughly the throughput gap.

## Observer cost: per-call, not aggregate

Tested by gating `record_observation` and `record_section_start` calls on an env-var-controlled `every-Nth-section` counter. Re-enabled the observer fully, ran /tiny at varying sample rates:

| N (every Nth) | req/s | vs N=1 | vs disabled (N=0) |
|---:|---:|---:|---:|
| 1 (every call) | 132,727 | — | -8.2% |
| 5 | 140,445 | +5.8% | -2.9% |
| 10 | 139,386 | +5.0% | -3.6% |
| 50 | 141,936 | +6.9% | -1.9% |
| 100 | 141,986 | +7.0% | -1.8% |
| 1,000 | 140,898 | +6.2% | -2.6% |
| 10,000 | 141,404 | +6.5% | -2.2% |
| 0 (disabled) | 144,625 | +9.0% | — |

**Curve plateaus at N≈5.** Going from "every call" to "every 5th" captures ~67% of the disabled-state win in one step. After that, throughput plateaus regardless of N. This means the observer's cost is overwhelmingly per-call fixed work (mutex lock + EMA computation + HashMap insert + eviction check) rather than aggregate work that scales with the volume of observations. The proper architectural fix — your sketch of a lock-free per-connection ring buffer that flushes periodically into the shared observer — should recover ~95% of the disabled-state perf while preserving full sampling accuracy.

The remaining ~2% gap (any-sampling-rate vs `N=0` early-return) is the cost of the global atomic counter `fetch_add` itself; making the counter per-connection would close it.

Sampling gate and observer-disable were experimental — both **reverted** before the headline sweep above. The shipping change in this session is just the hashbrown swap.

## Structural costs identified (not fixed)

### `encode_field_section_h3` Vec churn — needs a workspace-level buffer pool

`encode_field_section_h3` does, per response: `Vec::with_capacity(initial_cap=128)` for a temp scratch, encode field section into it, `buffer.resize(frame_header_len, 0)` to zero-fill the frame header slot, `buffer.extend_from_slice(&field_section_buf)` to memcpy the encoded section into the output buffer. The temp Vec exists because frame-header length isn't known until after encoding.

Profile attribution (children view):
- `encode_field_section_h3` total: 2.73% (= 2.25% self + 0.48% descendants)
- The descendants — actual QPACK encoder work — are only 0.48%. The QPACK encoding *itself* is fast.
- The 2.25% self is dominated by the Vec ops + their associated allocator work.

Local fixes are possible (fixed-stride frame header + in-place encode, or thread-local scratch) but the right shape is a workspace-level buffer pool: a `BufferPool::checkout(min_capacity)` returning a `PooledBuffer` that auto-returns its inner `Vec` on Drop, with `clear()` preserving capacity. Cross-cutting consumers: this site, the `output_buffer` Vec in `send_h3`, the request buffer in `process_inbound_bidi`, the h1 BufWriter body buffer (yesterday's "1MB body copies into BufWriter" finding), the h2 driver's per-stream `Buffer`. Real cross-crate API design — punted to a separate task.

### Spawn architecture — 3.34% across four sites

`TokioRuntime::spawn` (1.16%) + `Runtime::spawn(run_h3_connection)` (0.86%) + `Runtime::spawn(handle_bidi_stream)` (0.75%) + `spawn_inbound_uni_streams` (0.57%) = 3.34% of CPU on /tiny. Hyper has effectively one spawn per request. This is the same architectural choice noted in yesterday's h2 debrief — per-stream task spawn lets handler work parallelize across cores, which pays off on `/large/1m` (1.46x in h2; near-tied 0.88x in h3) but costs ~3% on small-response endpoints where there's nothing to parallelize. Known tradeoff.

### Recv path: same 3-pass copy as h2

`/recv1m` at 0.36x of hyper is the worst result in any of today's tables. Same structural cause as the h2 recv-path 3-pass copy noted in yesterday's debrief: trillium's `forbid(unsafe_code)` + `futures-lite` `&mut [u8]` API + non-bytes-crate stance forces 3 memcpys per body byte (driver read_buf → per-stream Buffer → handler `H2Transport::poll_read` slot → user `Vec::resize` zero-init). hyper's `Bytes`-refcounted path does ~1. Worth revisiting at a future major if `forbid(unsafe_code)` is relaxed.

## Cross-protocol summary: trillium vs hyper

Combining yesterday's h1.1 + h2 numbers with today's h3 numbers. All measured with `h2load` against the bench server, both stacks on jemalloc + aws-lc-rs + NODELAY-on-TCP, c7i.2xlarge loopback. trillium r/s as a fraction of hyper r/s:

| endpoint | h1.1 t/h | h2 t/h | h3 t/h | dominant story |
|---|---:|---:|---:|---|
| /tiny       | 0.78 | **1.06** | 0.60 | h3 regression vs h2 (observer + spawn overhead × per-request workload) |
| /1k         | 0.80 | **1.16** | 0.62 | same shape |
| /1m         | **0.49** | **1.46** | 0.88 | h1 BufWriter body copy (bad), h2 parallel-send pump (great), h3 bandwidth-bound |
| /echo64     | 0.79 | 0.71 | **1.57** | recv-path gap on h1/h2; h3 wins because hyper's h3 isn't CPU-saturated at this size |
| /echo16k    | 0.86 | 0.70 | 0.88 | recv-path gap |
| /echo1m     | 0.73 | 0.78 | 0.60 | recv-path gap (worse than 64b case because send-path also matters) |
| /recv64     | 0.78 | 0.71 | **1.59** | same as /echo64 |
| /recv16k    | **0.92** | 0.67 | 0.74 | h1 nearly tied (no per-stream Buffer), h2/h3 recv-path gap |
| /recv1m     | **0.96** | 0.63 | **0.36** | h1 essentially tied, h2 recv-path gap, h3 recv-path gap is worst |

Where trillium beats hyper:

- **h2 small responses (`/tiny`, `/small`)** — HPACK static-table fix landed yesterday; foldhash HashMap; specialized header encoding. 1.06–1.16x.
- **h2 bulk responses (`/large/1m`)** — parallel send pump across streams within a connection. 1.46x — the largest win in any table. h3 reduces to 0.88x because both stacks bottleneck on QUIC packet pacing rather than CPU.
- **h3 small `/echo` and `/recv` (64b)** — 1.57–1.59x. This is partly a CPU-saturation artifact: hyper's h3 stack only uses ~245% of 400% available CPU on these endpoints (something is bottlenecking it well below CPU limits — possibly h3-quinn's per-stream flow-control window churn at small body sizes). Trillium uses ~373% on the same workload. So this isn't "trillium is more efficient" — it's "trillium can use more cores when hyper-h3 can't". Real win, but with an asterisk.

Where trillium loses to hyper, in priority order:

1. **Recv path 1MB / large bulk uploads** — `/recv1m` at 0.36x h3, 0.63x h2, **but 0.96x h1**. The gap appears with multiplexed protocols (h2/h3) and disappears under h1. Root cause: h2/h3 driver buffers a per-stream copy of incoming body bytes (the 3-pass copy), while h1 streams body bytes directly from the transport. Locked behind `forbid(unsafe_code)` + `futures-lite` API + no-bytes-dep design choices. Not actionable without a major version break.

2. **h1.1 large responses** — `/large/1m` at **0.49x** h1, identified yesterday as the BufWriter body absorb path: a 1MB body goes through `extend_from_slice` into the response buffer rather than being vectored as `[buffered_headers, body_chunk]`. Concrete fix possible. Yesterday's debrief flagged this for follow-up.

3. **h3 small responses (`/tiny`, `/1k`)** — 0.60–0.62x. This is what we profiled today. The breakdown:
   - QPACK observer per-call cost (~2–3%) → batched/sampled redesign
   - 4 spawn sites (~3%) → architectural, possibly collapsible to 2
   - Vec churn in `encode_field_section_h3` (~2%) → buffer pool
   - async-fn state machine for orchestration (~3%) → not easily reducible

4. **h2 small responses on the recv path** (`/echo64`, `/recv64`, `/echo16k`, `/recv16k`) — 0.67–0.71x. Same recv-path 3-pass copy as #1 but with smaller bodies, so the impact is smaller in absolute terms.

The pattern across protocols is consistent: trillium is **competitive or winning on h2 send-heavy workloads**, **structurally penalized on multiplexed-protocol recv-heavy workloads** (h2/h3 recv path), and **broadly slower on small-response/per-request-overhead-dominated workloads** by 0.6–0.8x. The h2 send-side wins are real and shippable today; the recv-side gap is locked behind a future major version's design relaxation; the h3 small-response gap has actionable wins (observer redesign + buffer pool) that haven't yet been committed.

## What's next

In order of cleanest ROI:

1. **Buffer pool design** — workspace-level `BufferPool::checkout(min_capacity)` with auto-return on Drop. Used by `encode_field_section_h3`, `send_h3` output buffer, `process_inbound_bidi` request buffer, h1 BufWriter, h2 per-stream Buffer. Cross-cutting and worth doing once.

2. **QPACK observer redesign** — lock-free per-connection ring buffer that batches observations and flushes periodically into the shared HashMap+EMA. Sampling experiment validated the architecture. Should recover ~95% of /tiny's observer cost (~2% absolute throughput on /tiny, propagating modestly to other small-response endpoints).

3. **h1 BufWriter body bypass** — yesterday's flagged follow-up. Body chunks bypass the BufWriter and go through `poll_write_vectored` with `[buffered_headers, body_chunk]`. Closes a chunk of the 0.49x `/large/1m` h1 gap.

4. **Spawn collapse** — investigate whether `run_h3_connection` and `handle_bidi_stream` can share a task or whether `TokioRuntime::spawn` and `Runtime::spawn` are duplicating work. Probably ~1–2% on small-response endpoints. Architectural and risky — not until after #1 and #2.

Out of scope without a major version: recv-path 3-pass copy. Locked by `forbid(unsafe_code)` + futures-lite API + no-bytes-dep design choices.

## Methodology notes (delta from this morning)

- Built h2load with HTTP/3 from source under `~/build/h3-h2load/` — bash chain documented at top of debrief. Resulting binary is `~/build/h3-h2load/nghttp2/src/h2load`, `--h3` flag enables HTTP/3 over QUIC.
- `h3-sweep.sh` parallels `baseline.sh` and `h1-sweep.sh`. `--h3 -c 8 -m 10 --duration=8 --warm-up-time=1`. Builds both binaries fresh on each invocation.
- `sudo sh -c 'echo -1 > /proc/sys/kernel/perf_event_paranoid'` was needed before `perf record` worked (default 4 on this image).
- Run-to-run variance on /tiny is ~5–10% across full-sweep cycles, larger than expected. Single-shot ad-hoc measurements should be treated as suggestive, not conclusive — confirmed via the sweep harness with multiple endpoints sequentially.
- h2load reports `succeeded` and `2xx status` separately; for some h3 stream-close patterns these diverge by orders of magnitude. Always check both. Look at `2xx` count for true response throughput; `succeeded` is the right number when stream-level errors matter.
