#!/usr/bin/env bash
# Sweep h2_initial_stream_window_size against the /echo route.
# Compares lazy (0) vs RFC baseline (65535) vs eager (1MB) at fixed body sizes.
# Server is spawned and torn down per value to ensure a clean state.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/target/bench-release/trillium-bench-server"
PORT="${PORT:-8443}"
HOST="${HOST:-127.0.0.1}"
DURATION="${DURATION:-15}"
RESULTS_DIR="${RESULTS_DIR:-$ROOT/bench/results}"
RUN_LABEL="${RUN_LABEL:-window-sweep-$(date +%Y%m%d-%H%M%S)}"

mkdir -p "$RESULTS_DIR/$RUN_LABEL"

# 4 KiB body (one frame) and 64 KiB body (multi-frame, will need WINDOW_UPDATE
# topping up under lazy mode)
head -c 4096   /dev/zero | tr '\0' 'x' > /tmp/echo-4k.body
head -c 65536  /dev/zero | tr '\0' 'x' > /tmp/echo-64k.body

run_one() {
    local window="$1" body="$2" body_label="$3"
    local label="window-${window}-body-${body_label}"
    echo "=== $label ==="

    taskset -c 0-3 "$BIN" --port "$PORT" \
        --h2-initial-stream-window-size "$window" \
        > "$RESULTS_DIR/$RUN_LABEL/$label.server.log" 2>&1 &
    local server_pid=$!
    # Wait for the listener
    for _ in $(seq 1 50); do
        if (echo > /dev/tcp/127.0.0.1/$PORT) 2>/dev/null; then break; fi
        sleep 0.1
    done

    taskset -c 4-7 h2load -t 4 \
        --duration="$DURATION" --warm-up-time=2 \
        -c 4 -m 100 \
        -d "$body" \
        "https://$HOST:$PORT/echo" \
        > "$RESULTS_DIR/$RUN_LABEL/$label.h2load.txt" 2>&1 || true

    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
}

for window in 0 65535 1048576; do
    run_one "$window" /tmp/echo-4k.body  "4k"
    run_one "$window" /tmp/echo-64k.body "64k"
done

echo
echo "=== Summary ==="
for f in "$RESULTS_DIR/$RUN_LABEL"/*.h2load.txt; do
    label="$(basename "$f" .h2load.txt)"
    rps="$(grep -m1 'finished in' "$f" | awk '{for(i=1;i<=NF;i++)if($i=="req/s,")print $(i-1)}')"
    p99="$(grep 'time for request:' "$f" | awk 'NR==1{print $NF}')"
    echo "$label: $rps req/s   max=$p99"
done

echo "Results in $RESULTS_DIR/$RUN_LABEL"
