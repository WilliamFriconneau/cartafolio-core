#!/bin/sh
# Smoke test of an installed or freshly built memcp: setup, start, discovery, a connection, remember and recall, the
# page with a console key, doctor and backup. Used by CI after building the packages.
#   scripts/linux-smoke.sh [PATH-TO-memcp]      (default: target/release/memcp)
set -eu
BIN="${1:-target/release/memcp}"
PORT="${SMOKE_PORT:-18791}"
T=$(mktemp -d)
cleanup() { [ -z "${PID:-}" ] || kill "$PID" 2>/dev/null || true; rm -rf "$T"; }
trap cleanup EXIT INT TERM
fail() { echo "smoke: FAILED: $*" >&2; exit 1; }
export MEMCP_CONFIG="$T/memcp.toml"
unset MEMCP_DATA

"$BIN" setup --yes --data "$T/data" --config "$T/memcp.toml" --listen "127.0.0.1:$PORT" \
  --person Smoke --node-name smoke --language en >"$T/setup.out"
KEY=$(grep -o 'key=hcp_[A-Za-z0-9_]*' "$T/setup.out" | head -1 | cut -d= -f2)
CONSOLE=$(grep -o 'hcpc_[A-Za-z0-9_]*' "$T/setup.out" | head -1)
[ -n "$KEY" ] && [ -n "$CONSOLE" ] || fail "setup did not print the keys"
grep -q "http://127.0.0.1:$PORT/mcp/assistant?key=" "$T/setup.out" || fail "no address with a key"
[ "$(stat -c %a "$T/data")" = 700 ] || fail "the data directory is not private"

"$BIN" serve >"$T/serve.log" 2>&1 &
PID=$!
i=0
until curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; do
  i=$((i + 1)); [ $i -lt 50 ] || { cat "$T/serve.log" >&2; fail "the server did not start"; }
  sleep 0.2
done

curl -fsS "http://127.0.0.1:$PORT/.well-known/memcp" | grep -q smoke || fail "/.well-known/memcp"

call() { # call TOOL JSON-ARGS
  curl -fsS -X POST "http://127.0.0.1:$PORT/mcp/assistant" -H "Authorization: Bearer $KEY" \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}"
}
call remember '{"text":"2026-11-17, the smoke test wrote this.","space":"personal"}' | grep -q created || fail "remember"
call recall '{"query":"smoke test","space":"personal"}' | grep -q "wrote this" || fail "recall"

# A second connection, limited to one space, made the way the documentation says.
"$BIN" connect smoke-health --grant health:rw --public-url "http://127.0.0.1:$PORT" | grep -q 'hcp_' || fail "connect"

curl -fsS "http://127.0.0.1:$PORT/admin/api/state" -H "Authorization: Bearer $CONSOLE" | grep -q '"admin":true' || fail "the page"
CODE=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/admin/api/state" -H "Authorization: Bearer $KEY")
[ "$CODE" = 401 ] || fail "an assistant key opened the page ($CODE)"

"$BIN" doctor | tee "$T/doctor.out"
grep -q FAIL "$T/doctor.out" && fail "doctor found a problem"
"$BIN" backup --to "$T/backups" --keep 14 | grep -q '^backup: ' || fail "backup"
echo "smoke: ok"
