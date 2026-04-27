#!/usr/bin/env bash
# Head-to-head: glibc vs mimalloc vs jemalloc on /tiny throughput.
# Server pinned to cores 0-3, h2load to cores 4-7, TOKIO_WORKER_THREADS=4.
set -euo pipefail

BIN_DIR=/home/ubuntu/trillium/bench/binaries
RESULTS=/home/ubuntu/trillium/bench/results/allocator-shootout-$(date +%Y%m%d-%H%M%S)
mkdir -p "$RESULTS"

run_one() {
    local label="$1" bin="$2"
    echo "=== $label ==="

    TOKIO_WORKER_THREADS=4 nohup setsid taskset -c 0-3 "$bin" \
        > "$RESULTS/$label.server.log" 2>&1 &
    local server_pgid=$!
    sleep 1
    # find the actual server pid
    local server_pid
    server_pid=$(pgrep -f "$(basename "$bin")" | head -1)
    echo "server pid: $server_pid"

    # warm up + measure
    taskset -c 4-7 h2load -t 4 -c 4 -m 100 \
        --duration=10 --warm-up-time=2 \
        https://127.0.0.1:8443/tiny > "$RESULTS/$label.h2load.txt" 2>&1

    grep -E 'finished in|time for request' "$RESULTS/$label.h2load.txt"

    # Sample CPU during a follow-up short run
    (taskset -c 4-7 h2load -t 4 -c 4 -m 100 --duration=5 https://127.0.0.1:8443/tiny > /dev/null 2>&1) &
    local probe=$!
    sleep 2
    pidstat -u -p "$server_pid" 1 3 2>&1 | tail -5 | head -4 > "$RESULTS/$label.pidstat.txt"
    cat "$RESULTS/$label.pidstat.txt"
    wait $probe

    kill -TERM "$server_pid" 2>/dev/null
    sleep 1
    pkill -9 -f "$(basename "$bin")" 2>/dev/null
    sleep 0.5
    echo
}

run_one "glibc"    "$BIN_DIR/trillium-bench-server-glibc"
run_one "mimalloc" "$BIN_DIR/trillium-bench-server-mimalloc"
run_one "jemalloc" "$BIN_DIR/trillium-bench-server-jemalloc"

echo "==== Summary ===="
for label in glibc mimalloc jemalloc; do
    rps=$(grep -m1 'finished in' "$RESULTS/$label.h2load.txt" | awk -F'[ ,]+' '{for(i=1;i<=NF;i++)if($i ~ /req\/s/){print $(i-1); exit}}')
    p_max=$(grep 'time for request:' "$RESULTS/$label.h2load.txt" | awk '{print $4}')
    cpu=$(awk '/Average:/ {print $7}' "$RESULTS/$label.pidstat.txt" 2>/dev/null)
    printf "%-9s  %s req/s   max=%s  cpu=%s%%\n" "$label" "$rps" "$p_max" "$cpu"
done
echo "results in $RESULTS"
