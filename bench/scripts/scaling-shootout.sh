#!/usr/bin/env bash
# Run trillium (match-on-path) and hyper across c=[1,4,8,16,32], m=100.
# Both servers use jemalloc, max_concurrent_streams=100.
set -euo pipefail

BIN_DIR=/home/ubuntu/trillium/bench/binaries
RESULTS=/home/ubuntu/trillium/bench/results/scaling-$(date +%Y%m%d-%H%M%S)
mkdir -p "$RESULTS"

run_server_workload() {
    local label="$1" bin="$2"
    echo "## $label"

    pkill -9 -f "$(basename "$bin")" 2>/dev/null || true
    sleep 0.5
    TOKIO_WORKER_THREADS=4 nohup setsid taskset -c 0-3 "$bin" \
        > "$RESULTS/$label.server.log" 2>&1 &
    sleep 1
    local sp
    sp=$(pgrep -f "$(basename "$bin")\$" | head -1)
    echo "pid: $sp"

    for c in 1 4 8 16 32; do
        local out="$RESULTS/$label-c$c.h2load.txt"
        (taskset -c 4-7 h2load -t 4 -c $c -m 100 \
            --duration=8 --warm-up-time=1 \
            https://127.0.0.1:8443/tiny > "$out" 2>&1) &
        local pid=$!
        sleep 4
        # CPU sample
        pidstat -u -p "$sp" 1 3 2>&1 | tail -2 | head -1 \
            > "$RESULTS/$label-c$c.pidstat.txt"
        wait $pid
        local rps
        rps=$(grep -m1 'finished in' "$out" | awk -F'[ ,]+' '{for(i=1;i<=NF;i++)if($i ~ /req\/s/){print $(i-1); exit}}')
        local mean
        mean=$(grep 'time for request:' "$out" | awk '{print $5}')
        local maxl
        maxl=$(grep 'time for request:' "$out" | awk '{print $4}')
        local cpu
        cpu=$(awk '{print $7}' "$RESULTS/$label-c$c.pidstat.txt")
        printf "  c=%-2s   %s req/s   mean=%s  max=%s  cpu=%s%%\n" "$c" "$rps" "$mean" "$maxl" "$cpu"
    done

    pkill -9 -f "$(basename "$bin")" 2>/dev/null || true
    sleep 0.5
}

run_server_workload "trillium-mop"  "$BIN_DIR/trillium-bench-server-jemalloc-foldhash-mop"
run_server_workload "trillium-rtr"  "$BIN_DIR/trillium-bench-server-jemalloc-foldhash"
run_server_workload "hyper"         "$BIN_DIR/hyper-bench-server-jemalloc"

echo
echo "Results in $RESULTS"
