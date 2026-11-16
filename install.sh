#!/bin/sh
# MemCP installer: a guided install of the memory server on Linux (x86_64 or aarch64).
#
#   curl -fsSL https://raw.githubusercontent.com/WilliamFriconneau/memcp/main/install.sh | sh
#
# It downloads the release binaries, checks them against the published SHA256SUMS, makes a data folder and a
# first memory, and (if you want) runs the server as a service of your own user. Nothing is done as root, and
# nothing leaves this machine except the download. Read it first if you like: it is this file.
#
# Options (or the same words as environment variables, in capitals, prefixed MEMCP_):
#   --yes             take every default, ask nothing
#   --version vX.Y.Z  a release other than the latest
#   --prefix DIR      where the programs go (default ~/.local/bin)
#   --data DIR        where the memory lives (default ~/memory)
#   --language en|fr  the language of search and of the page (default en)
#   --spaces LIST     the first vaults, e.g. personal,health,home (default personal,health,home)
#   --port N          the port on the loopback (default 8791)
#   --no-service      do not make a systemd user service
#   --help
set -eu

REPO="${MEMCP_REPO:-WilliamFriconneau/memcp}"
BASE="${MEMCP_RELEASE_BASE:-https://github.com/$REPO/releases/download}"
API="${MEMCP_API:-https://api.github.com/repos/$REPO/releases/latest}"

VERSION="${MEMCP_VERSION:-}"
PREFIX="${MEMCP_PREFIX:-$HOME/.local/bin}"
DATA="${MEMCP_DATA:-$HOME/memory}"
LANGUAGE="${MEMCP_LANGUAGE:-en}"
SPACES="${MEMCP_SPACES:-personal,health,home}"
PORT="${MEMCP_PORT:-8791}"
SERVICE="${MEMCP_SERVICE:-ask}"
YES="${MEMCP_YES:-0}"

while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1 ;;
    --version) VERSION="$2"; shift ;;
    --prefix) PREFIX="$2"; shift ;;
    --data) DATA="$2"; shift ;;
    --language) LANGUAGE="$2"; shift ;;
    --spaces) SPACES="$2"; shift ;;
    --port) PORT="$2"; shift ;;
    --no-service) SERVICE=no ;;
    --help|-h) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

say()  { printf '%s\n' "$*"; }
step() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Questions are read from the terminal, so that `curl … | sh` can still ask them.
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

# --- what we run on ------------------------------------------------------------------------------------------
[ "$(uname -s)" = Linux ] || die "this installer is for Linux. On macOS, use: cargo install --git https://github.com/$REPO memcp-server (see the README). On Home Assistant, add the repository as an add-on repository."
case "$(uname -m)" in
  x86_64|amd64) TARGET=x86_64-unknown-linux-musl ;;
  aarch64|arm64) TARGET=aarch64-unknown-linux-musl ;;
  *) die "no binary for $(uname -m); build from source: cargo install --git https://github.com/$REPO memcp-server" ;;
esac
if have curl; then GET() { curl -fsSL "$1"; }; GETO() { curl -fsSL -o "$2" "$1"; }
elif have wget; then GET() { wget -qO- "$1"; }; GETO() { wget -qO "$2" "$1"; }
else die "curl or wget is needed"; fi
have sha256sum || die "sha256sum is needed to check the download"

# --- choices -------------------------------------------------------------------------------------------------
step "1/5  Choices"
ask "Where should the programs go?" "$PREFIX"; PREFIX="$ANSWER"
ask "Where should the memory live? (a folder of databases: back it up, move it, it is yours)" "$DATA"; DATA="$ANSWER"
ask "Language of search and of the page (en or fr)?" "$LANGUAGE"; LANGUAGE="$ANSWER"
ask "First vaults, comma separated (a vault is one box for one part of your life)" "$SPACES"; SPACES="$ANSWER"
ask "Port on this machine (the server listens on the loopback only)?" "$PORT"; PORT="$ANSWER"
case "$PORT" in ''|*[!0-9]*) die "the port must be a number" ;; esac

# --- download and verify -------------------------------------------------------------------------------------
step "2/5  Download"
if [ -z "$VERSION" ]; then
  VERSION=$(GET "$API" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1) || true
  [ -n "$VERSION" ] || die "cannot find the latest release (is the repository public, and are you online?)"
fi
say "release $VERSION for $TARGET"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT INT TERM
GETO "$BASE/$VERSION/SHA256SUMS" "$TMP/SHA256SUMS" || die "no SHA256SUMS for $VERSION"
for prog in memcp memcp-manager memcp-janitor; do
  f="$prog-$VERSION-$TARGET"
  say "  $f"
  GETO "$BASE/$VERSION/$f" "$TMP/$f" || die "cannot download $f"
  want=$(grep " $f\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)
  [ -n "$want" ] || die "$f is not in SHA256SUMS"
  got=$(sha256sum "$TMP/$f" | cut -d' ' -f1)
  [ "$want" = "$got" ] || die "checksum mismatch for $f: refusing to install it"
done
say "checksums match."

mkdir -p "$PREFIX"
for prog in memcp memcp-manager memcp-janitor; do
  install -m 755 "$TMP/$prog-$VERSION-$TARGET" "$PREFIX/$prog"
done
say "installed in $PREFIX"
case ":$PATH:" in *":$PREFIX:"*) ;; *) say "note: $PREFIX is not in your PATH; add it to your shell profile." ;; esac

# --- the memory ----------------------------------------------------------------------------------------------
step "3/5  Your memory"
if [ -e "$DATA/config.toml" ]; then
  say "$DATA already holds a memory: left as it is."
else
  "$PREFIX/memcp" --data "$DATA" init --spaces "$SPACES" --language "$LANGUAGE"
fi
chmod 700 "$DATA" 2>/dev/null || true

# --- the service ---------------------------------------------------------------------------------------------
step "4/5  Run it"
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
ExecStart=$PREFIX/memcp --data $DATA serve --listen 127.0.0.1:$PORT
Restart=on-failure
RestartSec=5
# The server only needs its data folder.
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
  say "Start it by hand when you want:"
  say "  $PREFIX/memcp --data $DATA serve --listen 127.0.0.1:$PORT"
fi

# --- the first key -------------------------------------------------------------------------------------------
step "5/5  Open the page"
TOKEN=$("$PREFIX/memcp" --data "$DATA" console-token owner 2>/dev/null | grep -o 'hcpc_[A-Za-z0-9_]*' | head -1 || true)
say "The administration page is on this machine only:"
say "  http://127.0.0.1:$PORT/admin"
if [ -n "$TOKEN" ]; then
  say "Paste this console key there (shown once, it opens the page and never the memory):"
  say "  $TOKEN"
fi

cat <<EOF

Next, to connect an assistant:
  1. Put HTTPS in front of port $PORT, because keys travel in clear on plain HTTP.
       Tailscale:  tailscale serve --bg $PORT      (private, your devices only)
       Caddy:      memory.example.net { reverse_proxy 127.0.0.1:$PORT }
  2. Make a key for one assistant, limited to what it needs:
       $PREFIX/memcp --data $DATA connect claude-code --grant work:rw --public-url https://YOUR-ADDRESS
     It prints the key once, and the line to paste into the assistant.
  3. Claude on the web or phone connects from the page: "Connect an assistant".

Backup:  $PREFIX/memcp --data $DATA backup --to ~/memory-backups
Update:  run this installer again; your memory is left alone.
Docs:    https://github.com/$REPO#readme
EOF
