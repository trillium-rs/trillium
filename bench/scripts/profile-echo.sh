#!/usr/bin/env bash
set +e

pkill -9 -f trillium-bench 2>/dev/null
sleep 0.5
echo "[stage1] cleanup done"

TOKIO_WORKER_THREADS=4 nohup setsid taskset -c 0-3 \
    /home/ubuntu/trillium/bench/binaries/trillium-bench-server-jemalloc-foldhash-mop \
    > /tmp/trill-srv.log 2>&1 &
disown
sleep 1
SRV_PID=$(pgrep -f "trillium-bench-server-jemalloc-foldhash-mop\$" | head -1)
echo "[stage2] server pid: $SRV_PID"
ps -p "$SRV_PID" -o pid,state,comm

# Workload
taskset -c 4-7 h2load -t 4 -c 8 -m 10 \
    --duration=10 --warm-up-time=1 \
    -d /tmp/echo-16k https://127.0.0.1:8443/echo > /tmp/h2load.log 2>&1 &
H2L_PID=$!
sleep 2
echo "[stage3] h2load pid: $H2L_PID"

# perf
echo "[stage4] starting perf record..."
perf record -F 999 -g --call-graph=dwarf,16384 -p "$SRV_PID" -o /tmp/trill-echo16k.data -- sleep 6 > /tmp/perf.log 2>&1
PERF_RC=$?
echo "[stage5] perf exit: $PERF_RC"
echo "--- perf.log ---"
cat /tmp/perf.log

wait "$H2L_PID"
echo "[stage6] h2load done"
grep -E 'finished in|status codes' /tmp/h2load.log
kill -9 "$SRV_PID" 2>/dev/null
sleep 0.3
ls -la /tmp/trill-echo16k.data 2>&1
echo "[done]"
