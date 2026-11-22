#!/bin/sh
# Pensine installer: a guided install of the memory server on Linux (x86_64 or aarch64).
#
#   curl -fsSL https://raw.githubusercontent.com/WilliamFriconneau/pensine-core/main/install.sh | sh
#   sudo sh install.sh --system        a system service run by its own user (see docs/LINUX.md)
#
# It downloads the release archive, checks it against the published SHA256SUMS, installs the programs, then lets
# `memcp setup` make the memory (private data folder, first person, default spaces, memcp.toml) and prints the
# address of the page and a key for an assistant that takes one. Nothing leaves this machine except the download.
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

REPO="${PENSINE_REPO:-${SILICE_REPO:-${MEMCP_REPO:-WilliamFriconneau/pensine-core}}}"
BASE="${PENSINE_RELEASE_BASE:-${SILICE_RELEASE_BASE:-${MEMCP_RELEASE_BASE:-https://github.com/$REPO/releases/download}}}"
API="${PENSINE_API:-${SILICE_API:-${MEMCP_API:-https://api.github.com/repos/$REPO/releases/latest}}}"

VERSION="${PENSINE_VERSION:-${SILICE_VERSION:-${MEMCP_VERSION:-}}}"
PREFIX="${PENSINE_PREFIX:-${SILICE_PREFIX:-${MEMCP_PREFIX:-}}}"
DATA="${PENSINE_DATA:-${SILICE_DATA:-${MEMCP_DATA:-}}}"
LANGUAGE="${PENSINE_LANGUAGE:-${SILICE_LANGUAGE:-${MEMCP_LANGUAGE:-en}}}"
SPACES="${PENSINE_SPACES:-${SILICE_SPACES:-${MEMCP_SPACES:-}}}"
PORT="${PENSINE_PORT:-${SILICE_PORT:-${MEMCP_PORT:-8791}}}"
SERVICE="${PENSINE_SERVICE:-${SILICE_SERVICE:-${MEMCP_SERVICE:-ask}}}"
SYSTEM="${PENSINE_SYSTEM:-${SILICE_SYSTEM:-${MEMCP_SYSTEM:-0}}}"
YES="${PENSINE_YES:-${SILICE_YES:-${MEMCP_YES:-0}}}"

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

step "Pensine installer"
say "A durable memory for your AI assistants, kept on your machine."

[ "$(uname -s)" = Linux ] || die "this installer is for Linux. On macOS, use: cargo install --git https://github.com/$REPO memcp-server (see the README). On Home Assistant, add the repository as an add-on repository."
case "$(uname -m)" in
  x86_64|amd64) TARGET=x86_64-unknown-linux-musl; SHORT=linux-x64 ;;
  aarch64|arm64) TARGET=aarch64-unknown-linux-musl; SHORT=linux-arm64 ;;
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
  f="pensine-${VERSION#v}-$SHORT.tar.gz"
  if ! GETO "$BASE/$VERSION/$f" "$TMP/$f" 2>/dev/null; then
    f="memcp-${VERSION#v}-$TARGET.tar.gz"
    GETO "$BASE/$VERSION/$f" "$TMP/$f" || die "cannot download $f"
  fi
  want=$(grep " $f\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)
  [ -n "$want" ] || die "$f is not in SHA256SUMS"
  got=$(sha256sum "$TMP/$f" | cut -d' ' -f1)
  [ "$want" = "$got" ] || die "checksum mismatch for $f: refusing to install it"
  say "checksum matches."
  tar -C "$TMP" -xzf "$TMP/$f"
  SRC="$TMP/${f%.tar.gz}"
  [ -x "$SRC/bin/memcp" ] || die "the archive does not hold bin/memcp"
fi

mkdir -p "$PREFIX"
PREFIX=$(cd "$PREFIX" && pwd)
for prog in memcp memcp-janitor; do
  install -m 755 "$SRC/bin/$prog" "$PREFIX/$prog"
done
for prog in memcp-manager memcp-sync; do
  if [ -x "$SRC/bin/$prog" ]; then install -m 755 "$SRC/bin/$prog" "$PREFIX/$prog"; fi
done
for prog in memcp memcp-manager memcp-sync; do
  if [ -x "$PREFIX/$prog" ]; then ln -sfn "$prog" "$PREFIX/pensine${prog#memcp}"; fi
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
  set -- "$PREFIX/memcp" setup --system
else
  CONF="${XDG_CONFIG_HOME:-$HOME/.config}/memcp/memcp.toml"
  set -- "$PREFIX/memcp" setup
fi
set -- "$@" --listen "$LISTEN" --language "$LANGUAGE" --data "$DATA" --config "$CONF"
[ -z "$SPACES" ] || set -- "$@" --spaces "$SPACES"
# With curl piped to sh, setup reads its answers from the terminal.
if [ "$YES" != 1 ] && ( : </dev/tty ) 2>/dev/null; then "$@" </dev/tty; else "$@" --yes; fi

# Quote one systemd word and escape literal specifiers. Unit values are single-line.
unit_word() {
  case "$1" in *'
'*) die "a systemd path cannot contain a newline" ;; esac
  printf '"'
  printf '%s' "$1" | sed 's/[\\"]/\\&/g; s/%/%%/g'
  printf '"'
}
sed_replacement() { printf '%s' "$1" | sed 's/[\\&#]/\\&/g'; }

# --- the service -----------------------------------------------------------------------------------------------
step "3/4  Run it"
if [ "$SYSTEM" = 1 ]; then
  UNIT_DIR=/etc/systemd/system
  if [ "$SERVICE" = ask ]; then
    if have systemctl && [ -d /run/systemd/system ]; then
      if confirm "Start Pensine now and at every boot?" y; then SERVICE=yes; else SERVICE=no; fi
    else SERVICE=no; fi
  fi
  for u in "$SRC"/units/*.service "$SRC"/units/*.timer; do install -m 644 "$u" "$UNIT_DIR/$(basename "$u")"; done
  install -m 644 "$SRC/units/memcp.tmpfiles" /usr/lib/tmpfiles.d/memcp.conf
  install -m 644 "$SRC/units/memcp.sysusers" /usr/lib/sysusers.d/memcp.conf
  if have systemd-tmpfiles; then systemd-tmpfiles --create /usr/lib/tmpfiles.d/memcp.conf || true; fi
  # Change only the executable word in the service units shipped with this archive.
  if [ "$PREFIX" != /usr/bin ]; then
    server=$(unit_word "$PREFIX/memcp")
    janitor=$(unit_word "$PREFIX/memcp-janitor")
    for u in "$SRC"/units/*.service; do
      sed -i -e "s#^\(ExecStart[A-Za-z]*=\)/usr/bin/memcp #\1$(sed_replacement "$server") #" \
        -e "s#^\(ExecStart[A-Za-z]*=\)/usr/bin/memcp-janitor #\1$(sed_replacement "$janitor") #" \
        "$UNIT_DIR/$(basename "$u")"
    done
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
      if confirm "Run Pensine as a service of your user, started at login?" y; then SERVICE=yes; else SERVICE=no; fi
    else SERVICE=no; fi
  fi
  if [ "$SERVICE" = yes ]; then
    executable=$(unit_word "$PREFIX/memcp")
    environment=$(unit_word "MEMCP_CONFIG=$CONF")
    writable=$(unit_word "$DATA")
    mkdir -p "$UNIT_DIR"
    cat >"$UNIT_DIR/memcp.service" <<EOF
[Unit]
Description=Pensine, a durable memory for AI assistants
After=network-online.target

[Service]
Environment=$environment
ExecStart=$executable serve
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=$writable
PrivateTmp=true

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable --now memcp.service
    have loginctl && loginctl enable-linger "$USER" 2>/dev/null || say "tip: run 'loginctl enable-linger $USER' to keep it running when you are logged out."
    say "service started: systemctl --user status memcp"
  else
    say "Start it by hand when you want:  MEMCP_CONFIG=\"$CONF\" \"$PREFIX/memcp\" serve"
  fi
fi

step "4/4  Next"
cat <<EOF
Check the installation:   "$PREFIX/pensine" doctor
Commands: pensine, pensine-manager and pensine-sync are installed in $PREFIX with their silice and memcp equivalents.
Backups, TLS, upgrade:    docs/LINUX.md  (https://github.com/$REPO/blob/main/docs/LINUX.md)
Put HTTPS in front of port $PORT before an assistant outside this machine connects (keys travel in clear on plain HTTP):
  Tailscale:  tailscale serve --bg $PORT      (private, your devices only)
  Caddy:      memory.example.net { reverse_proxy 127.0.0.1:$PORT }
For an assistant that signs in, use Connect an assistant on your page and enter its one-time code on the public consent page.
Assistants that take a key and devices use their own keys.
Update:  run this installer again; your memory and your memcp.toml are left alone.
EOF
