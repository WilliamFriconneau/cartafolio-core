#!/bin/sh
# MemCP installer: a guided install of the memory server on Linux (x86_64 or aarch64).
#
#   curl -fsSL https://raw.githubusercontent.com/WilliamFriconneau/silice-core/main/install.sh | sh
#   sudo sh install.sh --system        a system service run by its own user (see docs/LINUX.md)
#
# It downloads the release archive, checks it against the published SHA256SUMS, installs the programs, then lets
# `memcp setup` make the memory (private data folder, first person, default spaces, memcp.toml) and prints the
# address of the page and the address with the key for an assistant. Nothing leaves this machine except the download.
# Read it first if you like: it is this file. Inside an unpacked archive it installs from the archive, offline.
#
# Options (or the same words as environment variables, in capitals, prefixed MEMCP_):
#   --system          install for the whole machine (as root): /usr/local/bin, /etc/memcp, a user `memcp`, systemd units
#   --yes             take every default, ask nothing
#   --version vX.Y.Z  a release other than the latest
#   --prefix DIR      where the programs go (default ~/.local/bin; /usr/local/bin with --system)
#   --data DIR        where the memory lives (default ~/memory; /var/lib/memcp with --system)
#   --language en|fr  the language of search and of the page (default en)
#   --spaces LIST     the first spaces, e.g. personal,health,home (default: the six usual ones)
#   --port N          the port on the loopback (default 8791)
#   --no-service      do not make a systemd service
#   --help
set -eu

REPO="${MEMCP_REPO:-WilliamFriconneau/silice-core}"
BASE="${MEMCP_RELEASE_BASE:-https://github.com/$REPO/releases/download}"
API="${MEMCP_API:-https://api.github.com/repos/$REPO/releases/latest}"

VERSION="${MEMCP_VERSION:-}"
PREFIX="${MEMCP_PREFIX:-}"
DATA="${MEMCP_DATA:-}"
LANGUAGE="${MEMCP_LANGUAGE:-en}"
SPACES="${MEMCP_SPACES:-}"
PORT="${MEMCP_PORT:-8791}"
SERVICE="${MEMCP_SERVICE:-ask}"
SYSTEM="${MEMCP_SYSTEM:-0}"
YES="${MEMCP_YES:-0}"

while [ $# -gt 0 ]; do
  case "$1" in
    --system) SYSTEM=1 ;;
    --yes|-y) YES=1 ;;
    --version) VERSION="$2"; shift ;;
    --prefix) PREFIX="$2"; shift ;;
    --data) DATA="$2"; shift ;;
    --language) LANGUAGE="$2"; shift ;;
    --spaces) SPACES="$2"; shift ;;
    --port) PORT="$2"; shift ;;
    --no-service) SERVICE=no ;;
    --help|-h) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

say()  { printf '%s\n' "$*"; }
step() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

if [ "$SYSTEM" = 1 ]; then
  [ "$(id -u)" = 0 ] || die "--system needs root: sudo sh install.sh --system"
  [ -n "$PREFIX" ] || PREFIX=/usr/local/bin
  [ -n "$DATA" ] || DATA=/var/lib/memcp
else
  [ -n "$PREFIX" ] || PREFIX="$HOME/.local/bin"
  [ -n "$DATA" ] || DATA="$HOME/memory"
fi

# Questions are read from the terminal, so that `curl ... | sh` can still ask them.
ask() { # ask "question" default -> answer in $ANSWER
  if [ "$YES" = 1 ] || ! [ -r /dev/tty ]; then ANSWER="$2"; return; fi
  printf '%s [%s] ' "$1" "$2" >/dev/tty
  IFS= read -r ANSWER </dev/tty || ANSWER=""
  [ -n "$ANSWER" ] || ANSWER="$2"
}
confirm() { # confirm "question" default(y|n)
  ask "$1 (y/n)" "$2"
  case "$ANSWER" in y|Y|yes|YES|o|O|oui) return 0 ;; *) return 1 ;; esac
}

step "MemCP installer"
say "A durable memory for your AI assistants, kept on your machine."

[ "$(uname -s)" = Linux ] || die "this installer is for Linux. On macOS, use: cargo install --git https://github.com/$REPO memcp-server (see the README). On Home Assistant, add the repository as an add-on repository."
case "$(uname -m)" in
  x86_64|amd64) TARGET=x86_64-unknown-linux-musl ;;
  aarch64|arm64) TARGET=aarch64-unknown-linux-musl ;;
  *) die "no binary for $(uname -m); build from source: cargo install --git https://github.com/$REPO memcp-server" ;;
esac

# --- the programs: from this archive if we are in one, else downloaded and checked -----------------------------
HERE=$(cd "$(dirname "$0")" 2>/dev/null && pwd || echo .)
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT INT TERM
if [ -x "$HERE/bin/memcp" ] && [ -d "$HERE/units" ]; then
  step "1/4  Programs (from this archive)"
  SRC="$HERE"
else
  if have curl; then GET() { curl -fsSL "$1"; }; GETO() { curl -fsSL -o "$2" "$1"; }
  elif have wget; then GET() { wget -qO- "$1"; }; GETO() { wget -qO "$2" "$1"; }
  else die "curl or wget is needed"; fi
  have sha256sum || die "sha256sum is needed to check the download"
  have tar || die "tar is needed"
  step "1/4  Download"
  if [ -z "$VERSION" ]; then
    VERSION=$(GET "$API" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1) || true
    [ -n "$VERSION" ] || die "cannot find the latest release (is the repository public, and are you online?)"
  fi
  say "release $VERSION for $TARGET"
  GETO "$BASE/$VERSION/SHA256SUMS" "$TMP/SHA256SUMS" || die "no SHA256SUMS for $VERSION"
  f="memcp-${VERSION#v}-$TARGET.tar.gz"
  GETO "$BASE/$VERSION/$f" "$TMP/$f" || die "cannot download $f"
  want=$(grep " $f\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)
  [ -n "$want" ] || die "$f is not in SHA256SUMS"
  got=$(sha256sum "$TMP/$f" | cut -d' ' -f1)
  [ "$want" = "$got" ] || die "checksum mismatch for $f: refusing to install it"
  say "checksum matches."
  tar -C "$TMP" -xzf "$TMP/$f"
  SRC="$TMP/memcp-${VERSION#v}-$TARGET"
  [ -x "$SRC/bin/memcp" ] || die "the archive does not hold bin/memcp"
fi

mkdir -p "$PREFIX"
for prog in memcp memcp-janitor; do
  install -m 755 "$SRC/bin/$prog" "$PREFIX/$prog"
done
for prog in memcp-manager memcp-sync; do
  if [ -x "$SRC/bin/$prog" ]; then install -m 755 "$SRC/bin/$prog" "$PREFIX/$prog"; fi
done
for prog in memcp memcp-manager memcp-sync; do
  if [ -x "$PREFIX/$prog" ]; then ln -sfn "$prog" "$PREFIX/silice${prog#memcp}"; fi
done
say "installed in $PREFIX"
case ":$PATH:" in *":$PREFIX:"*) ;; *) say "note: $PREFIX is not in your PATH; add it to your shell profile." ;; esac

# --- the memory ------------------------------------------------------------------------------------------------
step "2/4  Your memory"
LISTEN="127.0.0.1:$PORT"
case "$PORT" in ''|*[!0-9]*) die "the port must be a number" ;; esac
if [ "$SYSTEM" = 1 ]; then
  if ! getent passwd memcp >/dev/null 2>&1; then
    if have systemd-sysusers; then systemd-sysusers "$SRC/units/memcp.sysusers"
    else useradd --system --home-dir /var/lib/memcp --no-create-home --shell /usr/sbin/nologin memcp; fi
  fi
  install -d -m 755 /etc/memcp
  CONF=/etc/memcp/memcp.toml
  [ -e "$CONF" ] || install -m 644 "$SRC/memcp.toml" "$CONF.example"
  SETUP="$PREFIX/memcp setup --system --listen $LISTEN --language $LANGUAGE"
else
  CONF="${XDG_CONFIG_HOME:-$HOME/.config}/memcp/memcp.toml"
  SETUP="$PREFIX/memcp setup --listen $LISTEN --language $LANGUAGE"
fi
SETUP="$SETUP --data $DATA --config $CONF"
[ -z "$SPACES" ] || SETUP="$SETUP --spaces $SPACES"
[ "$YES" = 1 ] && SETUP="$SETUP --yes"
# `memcp setup` asks its own questions on the terminal; here they are asked on /dev/tty when piped from curl.
if [ "$YES" != 1 ] && [ -r /dev/tty ]; then $SETUP </dev/tty; else $SETUP --yes; fi

# --- the service -----------------------------------------------------------------------------------------------
step "3/4  Run it"
if [ "$SYSTEM" = 1 ]; then
  UNIT_DIR=/etc/systemd/system
  if [ "$SERVICE" = ask ]; then
    if have systemctl && [ -d /run/systemd/system ]; then
      if confirm "Start MemCP now and at every boot?" y; then SERVICE=yes; else SERVICE=no; fi
    else SERVICE=no; fi
  fi
  for u in "$SRC"/units/*.service "$SRC"/units/*.timer; do install -m 644 "$u" "$UNIT_DIR/$(basename "$u")"; done
  install -m 644 "$SRC/units/memcp.tmpfiles" /usr/lib/tmpfiles.d/memcp.conf
  install -m 644 "$SRC/units/memcp.sysusers" /usr/lib/sysusers.d/memcp.conf
  if have systemd-tmpfiles; then systemd-tmpfiles --create /usr/lib/tmpfiles.d/memcp.conf || true; fi
  # The units start the binaries in /usr/bin: point them at the prefix chosen.
  if [ "$PREFIX" != /usr/bin ]; then
    for u in "$UNIT_DIR"/memcp*.service; do sed -i "s#/usr/bin/memcp#$PREFIX/memcp#g" "$u"; done
  fi
  if [ "$SERVICE" = yes ]; then
    systemctl daemon-reload
    systemctl enable --now memcp.service memcp-backup.timer
    say "service started: systemctl status memcp"
  else
    systemctl daemon-reload 2>/dev/null || true
    say "Start it when you want: systemctl enable --now memcp memcp-backup.timer"
  fi
else
  UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  if [ "$SERVICE" = ask ]; then
    if have systemctl && systemctl --user show-environment >/dev/null 2>&1; then
      if confirm "Run MemCP as a service of your user, started at login?" y; then SERVICE=yes; else SERVICE=no; fi
    else SERVICE=no; fi
  fi
  if [ "$SERVICE" = yes ]; then
    mkdir -p "$UNIT_DIR"
    cat >"$UNIT_DIR/memcp.service" <<EOF
[Unit]
Description=MemCP, a durable memory for AI assistants
After=network-online.target

[Service]
Environment=MEMCP_CONFIG=$CONF
ExecStart=$PREFIX/memcp serve
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=$DATA
PrivateTmp=true

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable --now memcp.service
    have loginctl && loginctl enable-linger "$USER" 2>/dev/null || say "tip: run 'loginctl enable-linger $USER' to keep it running when you are logged out."
    say "service started: systemctl --user status memcp"
  else
    say "Start it by hand when you want:  MEMCP_CONFIG=$CONF $PREFIX/memcp serve"
  fi
fi

step "4/4  Next"
cat <<EOF
Check the installation:   $PREFIX/memcp doctor
Backups, TLS, upgrade:    docs/LINUX.md  (https://github.com/$REPO/blob/main/docs/LINUX.md)
Put HTTPS in front of port $PORT before an assistant outside this machine connects (keys travel in clear on plain HTTP):
  Tailscale:  tailscale serve --bg $PORT      (private, your devices only)
  Caddy:      memory.example.net { reverse_proxy 127.0.0.1:$PORT }
Update:  run this installer again; your memory and your memcp.toml are left alone.
EOF
