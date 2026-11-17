#!/bin/sh
# The end-to-end tests with real processes: `memcp`, `memcp-manager`, `memcp-sync` and `memcp-capture` are started on
# ephemeral loopback ports, in temporary directories (crates/client/tests/real_nodes.rs). Linux only.
# Usage: scripts/e2e.sh [TEST_NAME_FILTER]
set -eu
cd "$(dirname "$0")/.."
echo "== programs"
cargo build -p memcp-server -p memcp-manager -p memcp-capture
cargo build -p memcp-client --features sync --bin memcp-sync
echo "== real nodes"
# Tests in parallel are fine: each one has its own directory and ports.
cargo test -p memcp-client --features sync --test real_nodes -- "$@"
