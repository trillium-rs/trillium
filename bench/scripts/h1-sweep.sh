#!/usr/bin/env bash
set +e

BIN_DIR=/home/ubuntu/trillium/bench/binaries
RESULTS=/home/ubuntu/trillium/bench/results/h1-sweep-$(date +%Y%m%d-%H%M%S)
mkdir -p "$RESULTS"
echo "results dir: $RESULTS"

[ -f /tmp/echo-64b ] || head -c 64 /dev/urandom > /tmp/echo-64b
[ -f /tmp/echo-16k ] || head -c 16384 /dev/urandom > /tmp/echo-16k
[ -f /tmp/echo-1m  ] || head -c 1048576 /dev/urandom > /tmp/echo-1m

# (label, path, body_arg). h1: c=80 keepalive connections, sequential per connection.
TARGETS=(
    "tiny     /tiny       "
    "1k       /small      "
    "1m       /large/1m   "
    "echo64   /echo       -d /tmp/echo-64b"
    "echo16k  /echo       -d /tmp/echo-16k"
    "echo1m   /echo       -d /tmp/echo-1m"
    "recv64   /recv       -d /tmp/echo-64b"
    "recv16k  /recv       -d /tmp/echo-16k"
    "recv1m   /recv       -d /tmp/echo-1m"
)

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

    local c=80
    for spec in "${TARGETS[@]}"; do
        local lbl path extra
        lbl=$(echo "$spec" | awk '{print $1}')
        path=$(echo "$spec" | awk '{print $2}')
        extra=$(echo "$spec" | awk '{$1=""; $2=""; print}' | sed 's/^  *//')
        local out="$RESULTS/$server_label-$lbl.h2load.txt"
        # h2load --h1 is shorthand for --alpn-list=http/1.1
        # shellcheck disable=SC2086
        taskset -c 4-7 h2load --h1 -t 4 -c $c \
            --duration=8 --warm-up-time=1 \
            $extra "https://127.0.0.1:8443${path}" > "$out" 2>&1 &
        local pid=$!
        sleep 4
        pidstat -u -p "$sp" 1 3 2>&1 | tail -2 | head -1 \
            > "$RESULTS/$server_label-$lbl.pidstat.txt"
        wait $pid

        local rps line mean cpu
        rps=$(grep -m1 'finished in' "$out" | grep -oE '[0-9.]+ req/s' | head -1 | awk '{print $1}')
        line=$(grep 'time for request:' "$out")
        mean=$(echo "$line" | awk '{print $6}')
        cpu=$(awk '{print $8}' "$RESULTS/$server_label-$lbl.pidstat.txt")
        printf "  %-8s  %10s req/s  mean=%-8s cpu=%s%%\n" "$lbl" "$rps" "$mean" "$cpu"
    done

    pkill -9 -f "$(basename "$bin")" 2>/dev/null
    sleep 0.5
}

run_one "trillium" "$BIN_DIR/trillium-bench-server-jemalloc-foldhash-mop"
echo
run_one "hyper"    "$BIN_DIR/hyper-bench-server-jemalloc"
echo
echo "Results in $RESULTS"
