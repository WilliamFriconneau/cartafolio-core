#!/bin/sh
# One command for the whole of what must be true before a push: format, lints, tests, audit of dependencies.
# CI runs the same steps. Usage: scripts/check.sh [quick]   (quick: format and lints only)
set -eu
cd "$(dirname "$0")/.."
echo "== format";  cargo fmt --all --check
echo "== lints";   cargo clippy --workspace --all-targets -- -D warnings
[ "${1:-}" = quick ] && exit 0
echo "== tests";   cargo test --workspace
if command -v cargo-deny >/dev/null 2>&1; then echo "== deny"; cargo deny check; fi
if command -v cargo-audit >/dev/null 2>&1; then echo "== audit"; cargo audit; fi
echo "ok"
