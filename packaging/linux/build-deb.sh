#!/bin/sh
# Builds memcp_<version>_<arch>.deb with cargo-deb. Run from anywhere; the package lands in target/debian/.
#   packaging/linux/build-deb.sh            native build
#   packaging/linux/build-deb.sh TARGET     for a Rust target (needs that target's toolchain), e.g. aarch64-unknown-linux-gnu
set -eu
cd "$(dirname "$0")/../.."
command -v cargo-deb >/dev/null 2>&1 || cargo install cargo-deb --locked
if [ $# -gt 0 ]; then
  cargo build --release --locked --target "$1" -p memcp-server -p memcp-janitor
  cargo deb --no-build --target "$1" -p memcp-server
else
  cargo build --release --locked -p memcp-server -p memcp-janitor
  cargo deb --no-build -p memcp-server
fi
ls -1 target/debian/*.deb target/*/debian/*.deb 2>/dev/null || true
