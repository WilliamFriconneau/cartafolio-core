#!/bin/sh
# 5S, "sort": look for what nothing uses. Read-only; prints findings, changes nothing.
# Usage: scripts/dead.sh   (needs cargo; installs nothing: cargo-machete and cargo-shear are used when present)
cd "$(dirname "$0")/.."
echo "== compiler: dead code and unused items"
touch crates/*/src/lib.rs crates/*/src/main.rs
cargo clippy --workspace --all-targets -- -W dead_code -W unused 2>&1 | grep -E "^warning" -A4 || echo "none"
for t in machete shear; do
  if command -v cargo-$t >/dev/null 2>&1; then echo "== cargo $t: unused or misplaced dependencies"; cargo $t || true
  else echo "== cargo $t is not installed (cargo install cargo-$t)"; fi
done
echo "== functions, consts and types named once in the whole workspace (public API of crates/capture-ffi is the Swift app's: keep)"
for n in $(grep -rhoE "^\s*pub(\([a-z]+\))? (const|static|fn|struct|enum|type|trait) [A-Za-z_0-9]+" --include=*.rs crates \
    | awk '{print $NF}' | sort -u); do
  [ "$(grep -rwo "$n" --include=*.rs --include=*.html --include=*.js crates | wc -l)" -le 1 ] && grep -rwln "$n" --include=*.rs crates | grep -v capture-ffi | sed "s|\$| $n|"
done || true
echo "== docs not linked from docs/README.md"
for f in docs/*.md; do b=$(basename "$f"); [ "$b" = README.md ] || grep -q "$b" docs/README.md || echo "$f"; done
echo "== forbidden words in code and docs (bridge, capture program, vault): only the names kept on purpose are listed in docs/FIVE-S.md"
grep -rniE "\b(bridge|vault)s?\b|capture (program|app)" --include=*.rs --include=*.md --include=*.html --include=*.sh crates docs ha-addon | grep -v "docs/reviews/\|RELAIS.md" | head -20 || true
