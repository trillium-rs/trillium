#!/usr/bin/env bash
# Run the trillium bench server pinned to cores 0-3.
# Pass any additional --flag args through to the server (HttpConfig knobs).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/target/bench-release/trillium-bench-server"

if [[ ! -x "$BIN" ]]; then
    echo "Server binary not found at $BIN" >&2
    echo "Build it with:" >&2
    echo "  RUSTFLAGS='-C force-frame-pointers=yes' cargo build --profile bench-release -p trillium-bench" >&2
    exit 1
fi

exec taskset -c 0-3 "$BIN" "$@"
