# trillium h2 bench debrief — 2026-04-27

Benchmarking session on `h2-dev` branch, c7i.2xlarge AWS instance (4 cores 0–3 server, 4 cores 4–7 load). All TLS via aws-lc-rs. jemalloc on both servers. Loopback (TCP_NODELAY enabled on both sides for fairness).

## Fixes shipped this session (unreleased h2-dev)

1. **h2 driver wake bug** (`acceptor.rs::park`, `acceptor/send.rs::has_pending_outbound_progress`). Driver was returning Pending after emitting body frames despite having budget and body to send — `park()` only checked handler signals, not pending outbound progress. Caused `/large/1m` and any response > ~49KB to stall after the first 3 frames. Fix: `park()` now also consults `has_pending_outbound_progress()` which returns true if any stream has a SendCursor in headers/trailers/complete phase, OR in body phase with `entry.send_window > 0` and `connection_send_window > 0`.

2. **HPACK static table lookup** (`headers/hpack/static_table/lookup.rs`). Replaced linear scan over 61-entry static table per header line with an `EntryName`-keyed match (compiler emits a jump table). Mirrors the structure of `headers/qpack/static_table/lookup.rs`. Saves ~2.2 percentage points of CPU under header-heavy workloads, ~6.8% throughput on /tiny.

3. **`HashMap` swap in h2 driver** to `hashbrown::HashMap` (foldhash) — done earlier in session, persisted. SipHash was visible in profile; foldhash crushed it and improved tail latency 5.2ms → 2.9ms in earlier runs.

4. **`bench/src/bin/trillium-server.rs`** uses `.with_nodelay()` explicitly. Trillium's server-common defaults `nodelay: false`. Surprising default — caused a 40ms delayed-ACK pathology on /echo16k (588 rps at 2% CPU before fix, 95k rps after). Worth changing at next major break.

## Final headline numbers (after all fixes, both NODELAY)

### h2 (h2load c=8 m=10)

| endpoint | trillium | hyper | t/h | story |
|---|---|---|---|---|
| /tiny | 347k r/s | 327k | **1.06x** | trillium wins (HPACK fix flipped it) |
| /small (1k) | 329k | 284k | **1.16x** | trillium wins |
| /large/1m | 6836 | 4683 | **1.46x** | parallel send pump |
| /large/10m | 799 | 739 | **1.08x** | bandwidth-bound (~7.4 GB/s, both servers near link cap) |
| /echo64 | 210k | 295k | 0.71x | recv-path gap |
| /echo16k | 95k | 136k | 0.70x | recv-path gap |
| /echo1m | 1863 | 2396 | 0.78x | recv loses but parallel send recovers some |
| /recv64 | 209k | 294k | 0.71x | recv-path gap |
| /recv16k | 122k | 182k | 0.67x | recv-path gap |
| /recv1m | 2920 | 4669 | 0.63x | recv-path gap, no parallel-send recovery |

### h1.1 (h2load --h1, c=80 keepalive)

| endpoint | trillium | hyper | t/h | story |
|---|---|---|---|---|
| /tiny | 265k | 338k | 0.78x | per-request overhead |
| /small (1k) | 260k | 326k | 0.80x | per-request overhead |
| /large/1m | 4811 | 9724 | **0.49x** | BufWriter body-copy + send-path efficiency |
| /echo64 | 257k | 327k | 0.79x | overhead |
| /echo16k | 107k | 125k | 0.86x | recv ~tied + send-path gap |
| /echo1m | 2397 | 3272 | 0.73x | send-path gap |
| /recv64 | 256k | 326k | 0.78x | per-request overhead |
| /recv16k | 139k | 151k | **0.92x** | nearly tied — h1 has no per-stream Buffer copy |
| /recv1m | 4758 | 4973 | **0.96x** | tied — h1 recv is competitive |

### h1.1 with `parse` feature (custom SIMD parser vs httparse default)

Within run-to-run noise on every endpoint (deltas ≤0.6%). The hand-rolled `memchr`-based parser that produces trillium types directly is competitive with httparse, because the conversion/cloning cost from httparse types to trillium types cancels httparse's parsing speed advantage.

## Structural costs identified (not fixed)

### h2 recv path: 3 passes per body byte

```
wire/rustls → driver read_buf (memcpy)
            → per-stream Buffer.extend_from_slice (memcpy: 1.42% of CPU)
            → handler H2Transport::poll_read copy_from_slice (memcpy: 1.04%)
            → user Vec slot (Vec::resize zero-init: 4.87% memset)
```

vs hyper's Bytes-refcounted path (essentially 1 pass).

**Why we can't fix this without breaking trillium principles:**
- Trillium is `forbid(unsafe_code)` (zero-unsafe, locked until next major)
- Trillium does not depend on `bytes` crate (not even as transitive dep elsewhere in the workspace)
- Trillium does not depend on tokio types (uses futures-lite traits)
- `futures-lite::AsyncRead::poll_read(cx, &mut [u8])` requires init memory — no uninit-aware variant in the futures ecosystem, only in tokio
- All three constraints together rule out: (a) `unsafe` uninit reads via `MaybeUninit`, (b) `BytesMut::spare_capacity_mut` + safe `BufMut`, (c) `tokio::io::ReadBuf` with `read_buf<B: BufMut>`

**Net:** the ~5–7% recv-path CPU cost is a structural consequence of the zero-unsafe + futures-lite + Vec API design choice. It's the most expensive choice in trillium's recv path but it's also load-bearing for the design philosophy. Worth revisiting at a future major if `forbid(unsafe_code)` is relaxed.

### h1 send path: 1MB body copy through BufWriter

`Conn::send` for h1.1 wraps the underlying transport in a `BufWriter` (`bufwriter.rs`) with `response_buffer_max_len = 2MB`. For a 1MB response body, the entire body gets `extend_from_slice`-ed into the BufWriter's buffer (1MB memcpy per response, visible as 2.89% memmove in profile), then flushed to the transport in one go.

`BufWriter::poll_write` already does vectored writes (`poll_write_vectored` with `[pending, additional]`) when the buffer would overflow capacity. But for a 1MB body inside a 2MB max, it never overflows — it just absorbs.

**Possible fix:** for body chunks specifically, bypass the buffer absorb path and unconditionally `poll_write_vectored` with `[buffered_headers, body_chunk]`. Headers (small, known size) still buffer; body chunks bypass. This would close some of the h1 /large/1m gap (currently 0.49x).

Not done in this session — flagged for follow-up. Worth investigating before declaring h1 send-path optimization complete.

### Per-stream task spawn cost

Trillium spawns a tokio task per h2 stream; hyper handles all streams in the per-connection task. Visible as ~1–3% memmove time across `spawn::run_h2` closure moves and `async_fn` future moves (Box allocation + copy of the future state machine).

This is architectural — trillium's design lets handler work parallelize across cores within a connection, which pays off on workloads like /large/1m (the 1.46x h2 win is largely this). The tradeoff is real per-request overhead at low concurrency. Net positive on most workloads.

## What's next

- **h1 BufWriter body bypass** — best ROI follow-up, could close a chunk of the h1 send gap
- **Chunked body benchmarking** — not done; needs a small Rust load generator (h2load can't do chunked uploads)
- **h2-dev landing** — wake bug fix needs a regression test before release; HPACK fix should be tested against more header shapes (currently has 51 unit tests passing)
- Potentially: a small benchmark harness committed to `bench/` so this kind of regression-tracking can be reproduced quickly in CI or by hand

## Methodology notes

- `nohup setsid taskset -c 0-3 BIN` for backgrounded server (Bash tool's run_in_background was unreliable for cross-call persistence)
- `perf record -F 999 -g --call-graph=dwarf,16384 -p PID -- sleep 6` for unwinding through libc (frame-pointer unwinding stops at libc boundaries)
- `samply --save-only -p PID` works but produces a JSON for the firefox profiler (not great for terminal triage; perf is better for that)
- `pidstat -u -p PID 1 3 | tail -2 | head -1` for CPU% sample mid-run
- Filter `--dsos BINARY` for userspace-only top symbols
- `--children` view to see kernel time aggregated under `sendto`/`recvmsg` callers
