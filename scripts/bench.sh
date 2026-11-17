#!/bin/sh
# The bench: real nodes (`memcp serve`), real passes (`memcp-sync`) and the command line, in a temporary directory, on
# loopback ports, driven from outside by `memcp-bench`: the swarm and simultaneous writes, load, security probes, kill -9.
# Prints a summary table; exits 1 when a check does not hold. macOS and Linux. See docs/BENCH.md.
# Usage: scripts/bench.sh [--only swarm,load,security,crash] [--notes N] [--writers N] [--clients N] [--secs N]
#                         [--settle N] [--keep]
set -eu
cd "$(dirname "$0")/.."
keep=0
for a in "$@"; do [ "$a" = --keep ] && keep=1; done
args=$(printf '%s\n' "$@" | grep -v '^--keep$' || true)
target=${CARGO_TARGET_DIR:-target}
echo "== programs (release)"
cargo build --release -p memcp-server -p memcp-bench
cargo build --release -p memcp-client --features sync --bin memcp-sync
work=$(mktemp -d "${TMPDIR:-/tmp}/memcp-bench.XXXXXX")
cleanup() {
  # Only the bench's own directory, and only one made by mktemp above.
  if [ "$keep" = 1 ]; then
    echo "kept: $work"
  else
    case "$work" in
      */memcp-bench.??????) rm -rf -- "$work" ;;
    esac
  fi
}
trap cleanup EXIT INT TERM
echo "== bench in $work"
status=0
# shellcheck disable=SC2086
"$target/release/memcp-bench" --bin "$target/release" --work "$work/run" --summary "$target/bench-summary.txt" $args || status=$?
exit "$status"
