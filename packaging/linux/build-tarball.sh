#!/bin/sh
# Builds memcp-<version>-<target>.tar.gz: the binaries, the units, the documented memcp.toml and install.sh.
#   packaging/linux/build-tarball.sh [TARGET]     TARGET defaults to the host (e.g. x86_64-unknown-linux-musl)
# The archive unpacks to one folder, memcp-<version>-<target>/, and `./install.sh` inside it installs from it.
set -eu
cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TARGET="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
cargo build --release --locked --target "$TARGET" -p memcp-server -p memcp-janitor -p memcp-manager -p memcp-client --features memcp-client/sync
NAME="memcp-$VERSION-$TARGET"
OUT="target/tarball/$NAME"
rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/units"
for b in memcp memcp-janitor memcp-manager memcp-sync; do install -m 755 "target/$TARGET/release/$b" "$OUT/bin/$b"; done
cp packaging/linux/*.service packaging/linux/*.timer packaging/linux/memcp.sysusers packaging/linux/memcp.tmpfiles "$OUT/units/"
cp packaging/linux/memcp.toml docs/LINUX.md LICENSE install.sh "$OUT/"
tar -C target/tarball -czf "target/tarball/$NAME.tar.gz" "$NAME"
echo "target/tarball/$NAME.tar.gz"
