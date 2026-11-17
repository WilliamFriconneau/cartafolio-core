#!/bin/sh
# Builds the static binaries the add-on image copies (amd64, aarch64). Needs `cross` and Docker.
set -eu
cd "$(dirname "$0")/.."
for pair in amd64:x86_64-unknown-linux-musl aarch64:aarch64-unknown-linux-musl; do
  arch=${pair%%:*}; target=${pair#*:}
  # One target directory per architecture, outside `target/`: cross runs the build scripts inside its container,
  # and it must never reuse the ones the host (or the other architecture) built against another glibc.
  export CARGO_TARGET_DIR="$PWD/target/cross-$arch"
  CROSS_CONTAINER_ENGINE=docker cross build --release --target "$target" -p memcp-server -p memcp-manager -p memcp-janitor
  CROSS_CONTAINER_ENGINE=docker cross build --release --target "$target" -p memcp-client --features sync --bin memcp-sync
  mkdir -p "ha-addon/memcp/bin/$arch"
  cp "$CARGO_TARGET_DIR/$target/release/memcp" "$CARGO_TARGET_DIR/$target/release/memcp-manager" "$CARGO_TARGET_DIR/$target/release/memcp-janitor" "$CARGO_TARGET_DIR/$target/release/memcp-sync" "ha-addon/memcp/bin/$arch/"
done
