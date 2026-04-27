#!/usr/bin/env bash
set +e

pkill -9 -f trillium-bench 2>/dev/null
sleep 0.5

TOKIO_WORKER_THREADS=4 nohup setsid taskset -c 0-3 \
    /home/ubuntu/trillium/bench/binaries/trillium-bench-server-jemalloc-foldhash-mop \
    > /tmp/trill-srv.log 2>&1 &
disown
sleep 1
SRV_PID=$(pgrep -f "trillium-bench-server-jemalloc-foldhash-mop\$" | head -1)
echo "[stage1] server pid: $SRV_PID"

taskset -c 4-7 h2load --h1 -t 4 -c 80 \
    --duration=10 --warm-up-time=1 \
    https://127.0.0.1:8443/large/1m > /tmp/h2load.log 2>&1 &
H2L_PID=$!
sleep 2

perf record -F 999 -g --call-graph=dwarf,16384 -p "$SRV_PID" -o /tmp/trill-h1-1m.data -- sleep 6 > /tmp/perf.log 2>&1
echo "perf rc: $?"

wait "$H2L_PID"
grep -E 'finished in' /tmp/h2load.log
kill -9 "$SRV_PID" 2>/dev/null
sleep 0.3
ls -la /tmp/trill-h1-1m.data
