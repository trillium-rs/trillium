#!/usr/bin/env bash
set -euo pipefail

BIN_DIR=/home/ubuntu/trillium/bench/binaries
RESULTS=/home/ubuntu/trillium/bench/results/endpoint-sweep-$(date +%Y%m%d-%H%M%S)
mkdir -p "$RESULTS"
echo "results dir: $RESULTS"

# (label, path, h2load extra args). echo uses POST + -d <file>, others GET.
TARGETS=(
    "tiny     /tiny       "
    "1k       /small      "
    "1m       /large/1m   "
    "10m      /large/10m  "
    "echo64   /echo       -d /tmp/echo-64b"
    "echo16k  /echo       -d /tmp/echo-16k"
    "echo1m   /echo       -d /tmp/echo-1m"
)

# echo bodies
[ -f /tmp/echo-64b ] || head -c 64 /dev/urandom > /tmp/echo-64b
[ -f /tmp/echo-16k ] || head -c 16384 /dev/urandom > /tmp/echo-16k
[ -f /tmp/echo-1m  ] || head -c 1048576 /dev/urandom > /tmp/echo-1m

run_one() {
    local server_label="$1" bin="$2"
    pkill -9 -f "$(basename "$bin")" 2>/dev/null || true
    sleep 0.5
    TOKIO_WORKER_THREADS=4 nohup setsid taskset -c 0-3 "$bin" \
        > "$RESULTS/$server_label.server.log" 2>&1 &
    sleep 1
    local sp
    sp=$(pgrep -f "$(basename "$bin")\$" | head -1)
    echo "## $server_label (pid $sp)"

    # Single concurrency level (c=8, m=10) — enough parallelism to saturate but few enough
    # streams to keep mean latency interpretable. Body sizes drive throughput differences.
    local c=8 m=10
    for spec in "${TARGETS[@]}"; do
        # parse: "label  path  extra…"
        local lbl path extra
        lbl=$(echo "$spec" | awk '{print $1}')
        path=$(echo "$spec" | awk '{print $2}')
        extra=$(echo "$spec" | awk '{$1=""; $2=""; print}' | sed 's/^  *//')

        local out="$RESULTS/$server_label-$lbl.h2load.txt"
        # shellcheck disable=SC2086
        (taskset -c 4-7 h2load -t 4 -c $c -m $m \
            --duration=8 --warm-up-time=1 \
            $extra "https://127.0.0.1:8443${path}" > "$out" 2>&1) &
        local pid=$!
        sleep 4
        pidstat -u -p "$sp" 1 3 2>&1 | tail -2 | head -1 \
            > "$RESULTS/$server_label-$lbl.pidstat.txt"
        wait $pid

        local rps bw line mean maxl cpu status
        rps=$(grep -m1 'finished in' "$out" | awk -F'[ ,]+' '{for(i=1;i<=NF;i++)if($i ~ /req\/s/){print $(i-1); exit}}')
        bw=$(grep -m1 'finished in' "$out"  | awk -F'[ ,]+' '{for(i=1;i<=NF;i++)if($i ~ /MB\/s/){print $(i-1); exit}}')
        line=$(grep 'time for request:' "$out")
        mean=$(echo "$line" | awk '{print $6}')
        maxl=$(echo "$line" | awk '{print $5}')
        cpu=$(awk '{print $8}' "$RESULTS/$server_label-$lbl.pidstat.txt")
        status=$(grep -m1 'status codes' "$out" | sed 's/^.*status codes: //')
        printf "  %-8s  %10s req/s  %7s MB/s  mean=%-8s max=%-8s cpu=%-6s  [%s]\n" \
            "$lbl" "$rps" "$bw" "$mean" "$maxl" "${cpu}%" "$status"
    done

    pkill -9 -f "$(basename "$bin")" 2>/dev/null || true
    sleep 0.5
}

run_one "trillium" "$BIN_DIR/trillium-bench-server-jemalloc-foldhash-mop"
echo
run_one "hyper"    "$BIN_DIR/hyper-bench-server-jemalloc"

echo
echo "Results in $RESULTS"
