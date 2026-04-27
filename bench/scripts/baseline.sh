#!/usr/bin/env bash
# Baseline benchmark — runs h2load against the trillium bench server with default config.
# Server is expected to already be running on $PORT (default 8443).
# Server should be pinned to cores 0-3 (taskset 0x0f) by run-server.sh.
# This script pins h2load to cores 4-7 (taskset 0xf0).
set -euo pipefail

PORT="${PORT:-8443}"
HOST="${HOST:-127.0.0.1}"
DURATION="${DURATION:-15}"
RESULTS_DIR="${RESULTS_DIR:-$(dirname "$0")/../results}"
RUN_LABEL="${RUN_LABEL:-baseline-$(date +%Y%m%d-%H%M%S)}"

mkdir -p "$RESULTS_DIR/$RUN_LABEL"

# h2load doesn't trust a self-signed cert by default; --ciphers and SNI are also
# explicit. -m default is 1 stream per conn; explicit for clarity.
H2LOAD_BASE="taskset -c 4-7 h2load --duration=$DURATION --warm-up-time=2"
URL_BASE="https://$HOST:$PORT"

run() {
    local name="$1" url="$2" c="$3" m="$4" extra="${5:-}"
    # h2load requires threads <= clients
    local t=$(( c < 4 ? c : 4 ))
    echo "=== $name ==="
    echo "url=$url c=$c m=$m t=$t extra=$extra"
    # shellcheck disable=SC2086
    $H2LOAD_BASE -t "$t" -c "$c" -m "$m" $extra "$url" 2>&1 \
        | tee "$RESULTS_DIR/$RUN_LABEL/$name.txt"
    echo
}

# Workload A: throughput on tiny response — protocol overhead dominates
run "A-tiny-c4m100"        "$URL_BASE/tiny"      4 100
run "A-tiny-c8m100"        "$URL_BASE/tiny"      8 100
run "A-tiny-c1m100"        "$URL_BASE/tiny"      1 100   # single-conn multiplex

# Workload B: throughput on medium body — exercises body write path
run "B-large-1m-c4m10"     "$URL_BASE/large/1m"  4 10
run "B-large-16k-c4m100"   "$URL_BASE/large/16k" 4 100

# Workload C: echo (POST) — exercises request body read and recv windows
head -c 1024 /dev/zero | tr '\0' 'x' > /tmp/echo-1k.body
run "C-echo-1k-c4m100"     "$URL_BASE/echo"      4 100 "-d /tmp/echo-1k.body"

echo "Results in $RESULTS_DIR/$RUN_LABEL"
