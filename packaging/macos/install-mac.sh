#!/bin/sh
# MemCP on a Mac: the node (`memcp serve`) and the bridge (`memcp-capture`) as background tasks of your user, started at
# login and again after a crash, with their logs. Nothing runs as root and nothing is put outside your home folder.
#
#   sh install-mac.sh               install or update (asks a few questions)
#   sh install-mac.sh --uninstall   stop and remove the programs and the tasks; your memory is kept
#
# What goes where:
#   ~/Library/Application Support/MemCP/bin      memcp, memcp-capture, memcp-janitor
#   ~/Library/Application Support/MemCP/data     the memory (a folder of databases: back it up, it is yours)
#   ~/Library/Application Support/MemCP/bridge   the bridge's settings and what waits to be sent
#   ~/Library/Logs/MemCP                         node.log, bridge.log, transcribe.log
#   ~/Library/LaunchAgents/org.memcp.*.plist     the tasks (launchd agents of your user)
#   Keychain, service memcp-capture               the keys of the bridge and of this Mac (never written in a file)
#
# Options (or the same words as environment variables, in capitals, prefixed MEMCP_):
#   --yes                take every default, ask nothing
#   --version vX.Y.Z     a release other than the latest
#   --from DIR           take memcp, memcp-capture and memcp-janitor from DIR (a build of your own) instead of a release
#   --language en|fr     the language of search and of the page (default en)
#   --spaces LIST        the first vaults (default personal,work,health)
#   --port N             the node's port on the loopback (default 8791)
#   --bridge-port N      the bridge's port on the loopback, for assistants (default 8792)
#   --whisper-model FILE voice notes read on this Mac by whisper.cpp (`brew install whisper-cpp ffmpeg`) with this model
#   --speech-api URL     or by a speech service of the OpenAI kind (key in MEMCP_SPEECH_API_KEY): the recordings leave
#                        this Mac for it
#   --no-agents          install the programs and the memory, start nothing
#   --uninstall          remove the tasks, the programs and the bridge's key; the memory and the logs are kept
#   --purge              with --uninstall: also delete the memory and the logs (asks first, unless --yes)
#   --help
set -eu

REPO="${MEMCP_REPO:-WilliamFriconneau/memcp-core}"
BASE="${MEMCP_RELEASE_BASE:-https://github.com/$REPO/releases/download}"
API="${MEMCP_API:-https://api.github.com/repos/$REPO/releases/latest}"

ROOT="${MEMCP_ROOT:-$HOME/Library/Application Support/MemCP}"
LOGS="${MEMCP_LOGS:-$HOME/Library/Logs/MemCP}"
AGENTS="${MEMCP_AGENTS:-$HOME/Library/LaunchAgents}"
VERSION="${MEMCP_VERSION:-}"
FROM="${MEMCP_FROM:-}"
LANGUAGE="${MEMCP_LANGUAGE:-en}"
SPACES="${MEMCP_SPACES:-personal,work,health}"
PORT="${MEMCP_PORT:-8791}"
BRIDGE_PORT="${MEMCP_BRIDGE_PORT:-8792}"
WHISPER_MODEL="${MEMCP_WHISPER_MODEL:-}"
SPEECH_API="${MEMCP_SPEECH_API:-}"
AGENTS_ON="${MEMCP_AGENTS_ON:-1}"
YES="${MEMCP_YES:-0}"
ACTION=install
PURGE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1 ;;
    --version) VERSION="$2"; shift ;;
    --from) FROM="$2"; shift ;;
    --language) LANGUAGE="$2"; shift ;;
    --spaces) SPACES="$2"; shift ;;
    --port) PORT="$2"; shift ;;
    --bridge-port) BRIDGE_PORT="$2"; shift ;;
    --whisper-model) WHISPER_MODEL="$2"; shift ;;
    --speech-api) SPEECH_API="$2"; shift ;;
    --no-agents) AGENTS_ON=0 ;;
    --uninstall) ACTION=uninstall ;;
    --purge) PURGE=1 ;;
    --help|-h) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

BIN="$ROOT/bin"
DATA="$ROOT/data"
BRIDGE="$ROOT/bridge"
NODE_URL="http://127.0.0.1:$PORT"
CONNECTION=desktop-bridge
BRIDGE_URL="$NODE_URL/mcp/$CONNECTION"
LABELS="org.memcp.node org.memcp.bridge org.memcp.transcribe"
DOMAIN="gui/$(id -u)"

say()  { printf '%s\n' "$*"; }
step() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }
ask() {
  if [ "$YES" = 1 ] || ! [ -r /dev/tty ]; then ANSWER="$2"; return; fi
  printf '%s [%s] ' "$1" "$2" >/dev/tty
  IFS= read -r ANSWER </dev/tty || ANSWER=""
  [ -n "$ANSWER" ] || ANSWER="$2"
}
confirm() {
  ask "$1 (y/n)" "$2"
  case "$ANSWER" in y|Y|yes|YES|o|O|oui) return 0 ;; *) return 1 ;; esac
}

[ "$(uname -s)" = Darwin ] || die "this installer is for macOS; on Linux use install.sh, on Windows packaging/windows/install.ps1"

stop_agents() {
  for label in $LABELS; do
    launchctl bootout "$DOMAIN/$label" 2>/dev/null || true
  done
}

# --- uninstall -----------------------------------------------------------------------------------------------
if [ "$ACTION" = uninstall ]; then
  step "Removing MemCP from this Mac"
  stop_agents
  for label in $LABELS; do rm -f "$AGENTS/$label.plist"; done
  security delete-generic-password -s memcp-capture -a "$BRIDGE_URL" >/dev/null 2>&1 || true
  security delete-generic-password -s memcp-capture -a "$NODE_URL/mcp/this-mac" >/dev/null 2>&1 || true
  rm -rf "$BIN" "$BRIDGE"
  say "the tasks, the programs and the bridge's key are removed."
  if [ "$PURGE" = 1 ]; then
    if [ "$YES" = 1 ] || confirm "Delete the memory in $DATA and the logs? This cannot be undone" n; then
      rm -rf "$DATA" "$LOGS"
      rmdir "$ROOT" 2>/dev/null || true
      say "the memory and the logs are deleted."
    else
      say "the memory is kept in $DATA."
    fi
  else
    say "your memory is kept in $DATA (delete it yourself, or run again with --purge)."
  fi
  exit 0
fi

step "MemCP for the Mac"
say "A durable memory for your AI assistants, kept on this Mac."

# --- choices -------------------------------------------------------------------------------------------------
step "1/5  Choices"
ask "Language of search and of the page (en or fr)?" "$LANGUAGE"; LANGUAGE="$ANSWER"
ask "First vaults, comma separated (a vault is one box for one part of your life)" "$SPACES"; SPACES="$ANSWER"
ask "Port of the node on this Mac?" "$PORT"; PORT="$ANSWER"
ask "Port of the bridge on this Mac, for assistants?" "$BRIDGE_PORT"; BRIDGE_PORT="$ANSWER"
for p in "$PORT" "$BRIDGE_PORT"; do
  case "$p" in ''|*[!0-9]*) die "a port must be a number" ;; esac
done
[ "$PORT" != "$BRIDGE_PORT" ] || die "the node and the bridge need two ports"
NODE_URL="http://127.0.0.1:$PORT"
BRIDGE_URL="$NODE_URL/mcp/$CONNECTION"

# --- the programs --------------------------------------------------------------------------------------------
step "2/5  Programs"
umask 077
mkdir -p "$BIN" "$DATA" "$BRIDGE" "$LOGS"
chmod 700 "$ROOT" "$DATA" "$BRIDGE"
PROGRAMS="memcp memcp-capture memcp-janitor"
if [ -n "$FROM" ]; then
  for prog in $PROGRAMS; do
    [ -x "$FROM/$prog" ] || die "$FROM/$prog is missing"
  done
  for prog in $PROGRAMS; do install -m 755 "$FROM/$prog" "$BIN/$prog"; done
  say "taken from $FROM"
else
  case "$(uname -m)" in
    arm64) TARGET=aarch64-apple-darwin ;;
    *) die "the releases are for Apple silicon; on this Mac build from source (cargo build --release) and use --from" ;;
  esac
  have curl || die "curl is needed"
  if [ -z "$VERSION" ]; then
    VERSION=$(curl -fsSL "$API" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1) || true
    [ -n "$VERSION" ] || die "cannot find the latest release (are you online?)"
  fi
  say "release $VERSION for $TARGET"
  TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT INT TERM
  curl -fsSL -o "$TMP/SHA256SUMS" "$BASE/$VERSION/SHA256SUMS" || die "no SHA256SUMS for $VERSION"
  for prog in $PROGRAMS; do
    f="$prog-$VERSION-$TARGET"
    curl -fsSL -o "$TMP/$f" "$BASE/$VERSION/$f" || die "cannot download $f"
    want=$(grep " $f\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)
    [ -n "$want" ] || die "$f is not in SHA256SUMS"
    got=$(shasum -a 256 "$TMP/$f" | cut -d' ' -f1)
    [ "$want" = "$got" ] || die "checksum mismatch for $f: refusing to install it"
  done
  say "checksums match."
  stop_agents
  for prog in $PROGRAMS; do install -m 755 "$TMP/$prog-$VERSION-$TARGET" "$BIN/$prog"; done
fi
say "installed in $BIN"

# --- the memory ----------------------------------------------------------------------------------------------
step "3/5  Your memory"
if [ -e "$DATA/config.toml" ]; then
  say "$DATA already holds a memory: left as it is."
else
  "$BIN/memcp" --data "$DATA" init --spaces "$SPACES" --language "$LANGUAGE"
fi

# --- the node, running ---------------------------------------------------------------------------------------
step "4/5  The node and the bridge's key"
plist() { # label, interval (0: always on), program arguments...
  label=$1; interval=$2; shift 2
  {
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key>
  <array>
EOF
    for a in "$@"; do
      a=$(printf '%s' "$a" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')
      printf '    <string>%s</string>\n' "$a"
    done
    cat <<EOF
  </array>
  <key>RunAtLoad</key><true/>
EOF
    if [ "$interval" = 0 ]; then
      echo "  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>"
      echo "  <key>ThrottleInterval</key><integer>10</integer>"
    else
      echo "  <key>StartInterval</key><integer>$interval</integer>"
    fi
    cat <<EOF
  <key>ProcessType</key><string>Background</string>
  <key>Umask</key><integer>63</integer>
  <key>StandardOutPath</key><string>$LOGS/${label#org.memcp.}.log</string>
  <key>StandardErrorPath</key><string>$LOGS/${label#org.memcp.}.log</string>
</dict>
</plist>
EOF
  } > "$AGENTS/$label.plist"
}
stop_agents
# Another program on the node's port would answer in its place.
if curl -fs -o /dev/null "$NODE_URL/health" 2>/dev/null || lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  die "port $PORT is taken by another program (lsof -nP -iTCP:$PORT); choose another with --port"
fi
mkdir -p "$AGENTS"
plist org.memcp.node 0 "$BIN/memcp" --data "$DATA" serve --listen "127.0.0.1:$PORT"
# The node runs from here on: as its task, or for the time of the install only (--no-agents).
NODE_PID=
if [ "$AGENTS_ON" = 1 ]; then
  launchctl bootstrap "$DOMAIN" "$AGENTS/org.memcp.node.plist"
else
  "$BIN/memcp" --data "$DATA" serve --listen "127.0.0.1:$PORT" >>"$LOGS/node.log" 2>&1 &
  NODE_PID=$!
fi
i=0
until curl -fs "$NODE_URL/health" >/dev/null 2>&1; do
  i=$((i + 1)); [ $i -lt 30 ] || die "the node does not answer on $NODE_URL: see $LOGS/node.log"
  sleep 1
done

CONSOLE=$("$BIN/memcp" --data "$DATA" console-token owner 2>/dev/null | grep -o 'hcpc_[A-Za-z0-9_]*' | head -1 || true)
[ -n "$CONSOLE" ] || die "no console key could be made"
# This Mac joins its own memory as a device, as a phone does: that makes the person's « À trier », where every capture
# goes. The device's key (it writes only to À trier, reads nothing) is kept in the Keychain under its address.
DEVICE_ACCOUNT="$NODE_URL/mcp/this-mac"
if ! security find-generic-password -s memcp-capture -a "$DEVICE_ACCOUNT" >/dev/null 2>&1; then
  CODE=$(curl -fsS -X POST -H "Authorization: Bearer $CONSOLE" -H "X-Requested-With: memcp" \
    -H "Content-Type: application/json" -d '{}' "$NODE_URL/admin/api/pairing" |
    sed -n 's/.*"code"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  [ -n "$CODE" ] || die "the node gave no pairing code"
  PAIRED=$(curl -fsS -X POST -H "Content-Type: application/json" \
    -d "{\"code\":\"$CODE\",\"device\":\"this-mac\"}" "$NODE_URL/pair") || die "the pairing of this Mac failed"
  DEVICE_KEY=$(printf '%s' "$PAIRED" | grep -o 'hcp_[A-Za-z0-9_-]*' | head -1)
  [ -n "$DEVICE_KEY" ] || die "no key in the pairing's answer"
  security add-generic-password -U -s memcp-capture -a "$DEVICE_ACCOUNT" -l "MemCP, this Mac" -w "$DEVICE_KEY"
  DEVICE_KEY=
  say "this Mac is paired with its memory: its captures go to À trier."
fi

# The bridge's own connection: the memory's tools for the assistants on this Mac, and the voice notes to read.
if security find-generic-password -s memcp-capture -a "$BRIDGE_URL" >/dev/null 2>&1; then
  say "the bridge already has its key in the Keychain."
else
  OUT=$("$BIN/memcp" --data "$DATA" connect "$CONNECTION" --all --public-url "$NODE_URL" 2>&1 ||
        "$BIN/memcp" --data "$DATA" connect "$CONNECTION" --all --rotate --public-url "$NODE_URL" 2>&1) ||
    die "no key could be made for the bridge: $OUT"
  KEY=$(printf '%s\n' "$OUT" | grep -o 'hcp_[A-Za-z0-9_-]*' | head -1)
  [ -n "$KEY" ] || die "no key in the node's answer"
  security add-generic-password -U -s memcp-capture -a "$BRIDGE_URL" -l "MemCP bridge" -w "$KEY"
  KEY=
  say "a key for the bridge is in the Keychain (service memcp-capture)."
fi
# The bridge's settings: the address only; the key stays in the Keychain.
{
  echo "# Written by install-mac.sh; the key is in the Keychain (service memcp-capture, account the url)."
  echo "url = $BRIDGE_URL"
  echo "outbox = $BRIDGE/outbox"
  if [ -n "$WHISPER_MODEL" ] || [ -n "$SPEECH_API" ]; then
    # The recordings of the person's À trier (`<person>-a-trier`, where the devices put them).
    echo "transcribe = owner-a-trier"
    echo "whisper_lang = $LANGUAGE"
  fi
  if [ -n "$WHISPER_MODEL" ]; then
    [ -f "$WHISPER_MODEL" ] || die "no model at $WHISPER_MODEL"
    echo "whisper_model = $WHISPER_MODEL"
    for c in whisper-cli ffmpeg; do
      p=$(command -v "$c" || true)
      [ -n "$p" ] || die "$c is needed for --whisper-model: brew install whisper-cpp ffmpeg"
      [ "$c" = whisper-cli ] && echo "whisper_cmd = $p" || echo "ffmpeg_cmd = $p"
    done
  elif [ -n "$SPEECH_API" ]; then
    echo "whisper_api_url = $SPEECH_API"
    [ -z "${MEMCP_SPEECH_API_KEY:-}" ] || echo "whisper_api_key = $MEMCP_SPEECH_API_KEY"
  fi
} > "$BRIDGE/bridge.conf"
chmod 600 "$BRIDGE/bridge.conf"

# --- the tasks -----------------------------------------------------------------------------------------------
step "5/5  Run it"
plist org.memcp.bridge 0 "$BIN/memcp-capture" gateway --config "$BRIDGE/bridge.conf" --listen "127.0.0.1:$BRIDGE_PORT"
if [ -n "$WHISPER_MODEL" ] || [ -n "$SPEECH_API" ]; then
  plist org.memcp.transcribe 300 "$BIN/memcp-capture" transcribe --config "$BRIDGE/bridge.conf"
else
  rm -f "$AGENTS/org.memcp.transcribe.plist"
fi
if [ "$AGENTS_ON" = 1 ]; then
  for label in org.memcp.bridge org.memcp.transcribe; do
    [ -f "$AGENTS/$label.plist" ] && launchctl bootstrap "$DOMAIN" "$AGENTS/$label.plist"
  done
  say "the node answers on $NODE_URL, the bridge on 127.0.0.1:$BRIDGE_PORT; they start again at login."
else
  [ -z "$NODE_PID" ] || kill "$NODE_PID" 2>/dev/null || true
  say "nothing left running (--no-agents). Start them with:"
  say "  launchctl bootstrap $DOMAIN $AGENTS/org.memcp.node.plist"
fi

TOKEN=$CONSOLE
cat <<EOF

The page of your memory, on this Mac only:
  $NODE_URL/admin
EOF
[ -z "$TOKEN" ] || printf 'Its console key (shown once; it opens the page, never the memory):\n  %s\n' "$TOKEN"
cat <<EOF

An assistant that runs a command reaches the memory through the bridge, knowing neither its address nor its key:
  $BIN/memcp-capture connect 127.0.0.1:$BRIDGE_PORT
For Claude Code:
  claude mcp add memcp -- "$BIN/memcp-capture" connect 127.0.0.1:$BRIDGE_PORT

Logs:     $LOGS
Update:   run this installer again; your memory is left alone.
Remove:   sh install-mac.sh --uninstall   (the memory is kept; add --purge to delete it)
EOF
