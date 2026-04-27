#!/usr/bin/env bash
# h3 sweep: trillium vs hyper(-baseline-via-h3-crate) over HTTP/3.
# Server pinned 0-3, load pinned 4-7. h2load with --h3 (built from source under
# ~/build/h3-h2load — needs HTTP/3-enabled h2load).
set +e

H2LOAD="${H2LOAD:-$HOME/build/h3-h2load/nghttp2/src/h2load}"
if [[ ! -x "$H2LOAD" ]]; then
    echo "h2load with HTTP/3 not found at $H2LOAD" >&2
    exit 1
fi

BIN_DIR=/home/ubuntu/trillium/target/bench-release
RESULTS=/home/ubuntu/trillium/bench/results/h3-sweep-$(date +%Y%m%d-%H%M%S)
mkdir -p "$RESULTS"
echo "results dir: $RESULTS"

[ -f /tmp/echo-64b ] || head -c 64 /dev/urandom > /tmp/echo-64b
[ -f /tmp/echo-16k ] || head -c 16384 /dev/urandom > /tmp/echo-16k
[ -f /tmp/echo-1m  ] || head -c 1048576 /dev/urandom > /tmp/echo-1m

# (label, path, body_arg). h3: c=8 conns × m=10 streams, matches yesterday's h2 baseline shape.
TARGETS=(
    "tiny     /tiny        "
    "1k       /small       "
    "16k      /large/16k   "
    "1m       /large/1m    "
    "echo64   /echo        -d /tmp/echo-64b"
    "echo16k  /echo        -d /tmp/echo-16k"
    "echo1m   /echo        -d /tmp/echo-1m"
    "recv64   /recv        -d /tmp/echo-64b"
    "recv16k  /recv        -d /tmp/echo-16k"
    "recv1m   /recv        -d /tmp/echo-1m"
)

C=8
M=10
DURATION=8
WARMUP=1

run_one() {
    local server_label="$1" bin="$2"
    pkill -9 -f "$(basename "$bin")" 2>/dev/null
    sleep 0.5
    TOKIO_WORKER_THREADS=4 nohup setsid taskset -c 0-3 "$bin" \
        > "$RESULTS/$server_label.server.log" 2>&1 &
    disown
    sleep 1
    local sp
    sp=$(pgrep -f "$(basename "$bin")\$" | head -1)
    echo "## $server_label (pid $sp)"

    for spec in "${TARGETS[@]}"; do
        local lbl path extra
        lbl=$(echo "$spec" | awk '{print $1}')
        path=$(echo "$spec" | awk '{print $2}')
        extra=$(echo "$spec" | awk '{$1=""; $2=""; print}' | sed 's/^  *//')
        local out="$RESULTS/$server_label-$lbl.h2load.txt"
        # shellcheck disable=SC2086
        taskset -c 4-7 "$H2LOAD" --h3 -t 4 -c "$C" -m "$M" \
            --duration="$DURATION" --warm-up-time="$WARMUP" \
            $extra "https://127.0.0.1:8443${path}" > "$out" 2>&1 &
        local pid=$!
        sleep 4
        pidstat -u -p "$sp" 1 3 2>&1 | tail -2 | head -1 \
            > "$RESULTS/$server_label-$lbl.pidstat.txt"
        wait $pid

        local rps mean cpu succeeded twoxx total
        rps=$(grep -m1 'finished in' "$out" | grep -oE '[0-9.]+ req/s' | head -1 | awk '{print $1}')
        # h2load --h3 reports per-request mean on the "request     :" row in the histogram block
        mean=$(awk '/^request +:/{print $7; exit}' "$out")
        cpu=$(awk '{print $8}' "$RESULTS/$server_label-$lbl.pidstat.txt")
        # accuracy check — total / succeeded / 2xx should align
        total=$(awk '/^requests:/{print $2; exit}' "$out")
        succeeded=$(awk '/^requests:/{for(i=1;i<=NF;i++) if($i=="succeeded,"){print $(i-1); exit}}' "$out")
        twoxx=$(awk '/^status codes:/{print $3; exit}' "$out")
        printf "  %-8s  %10s req/s  mean=%-9s cpu=%s%%  succ=%s/%s 2xx=%s\n" \
            "$lbl" "$rps" "$mean" "$cpu" "$succeeded" "$total" "$twoxx"
    done

    pkill -9 -f "$(basename "$bin")" 2>/dev/null
    sleep 0.5
}

# Build both binaries fresh so we run today's code (no stale binaries/ artifacts).
echo "## building"
( cd /home/ubuntu/trillium && \
  RUSTFLAGS='-C force-frame-pointers=yes' \
  cargo build --profile bench-release -p trillium-bench --features jemalloc \
      --bin trillium-bench-server 2>&1 | tail -3 )
( cd /home/ubuntu/trillium && \
  RUSTFLAGS='-C force-frame-pointers=yes' \
  cargo build --profile bench-release -p trillium-bench --features "jemalloc hyper-bench" \
      --bin hyper-bench-server 2>&1 | tail -3 )

run_one "trillium" "$BIN_DIR/trillium-bench-server"
echo
run_one "hyper"    "$BIN_DIR/hyper-bench-server"
echo
echo "Results in $RESULTS"
