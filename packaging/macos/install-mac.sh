#!/bin/sh
# MemCP on a Mac: the node (`memcp serve`) as a background task of your user, started at login and again after a crash,
# with its log. The node reads the voice notes and watches the folders itself, as `memcp.toml` says. Nothing runs as
# root and nothing is put outside your home folder.
#
#   sh install-mac.sh               install or update (asks a few questions)
#   sh install-mac.sh --uninstall   stop and remove the programs and the tasks; your memory is kept
#
# What goes where:
#   ~/Library/Application Support/MemCP/bin      memcp, memcp-janitor
#   ~/Library/Application Support/MemCP/data     the memory (a folder of databases: back it up, it is yours)
#   ~/Library/Application Support/MemCP/memcp.toml  the node's settings: voice notes, watched folders (mode 600)
#   ~/Library/Logs/MemCP                         node.log
#   ~/Library/LaunchAgents/org.memcp.node.plist  the task (a launchd agent of your user)
#   Keychain, service memcp                      the keys of the assistants' connection and of this Mac (never in a file)
#
# Options (or the same words as environment variables, in capitals, prefixed MEMCP_):
#   --yes                take every default, ask nothing
#   --version vX.Y.Z     a release other than the latest
#   --from DIR           take memcp and memcp-janitor from DIR (a build of your own) instead of a release
#   --language en|fr     the language of search and of the page (default en)
#   --spaces LIST        the first vaults (default personal,work,health)
#   --port N             the node's port on the loopback (default 8791)
#   --bridge-port N      no longer used (the node serves the assistants itself); accepted, so that a script keeps working
#   --whisper-model FILE voice notes read on this Mac by whisper.cpp (`brew install whisper-cpp ffmpeg`) with this model
#   --speech-api URL     or by a speech service of the OpenAI kind (key in MEMCP_SPEECH_API_KEY): the recordings leave
#                        this Mac for it
#   --no-agents          install the programs and the memory, start nothing
#   --uninstall          remove the task, the programs and the keys of the Keychain; the memory and the logs are kept
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
# `MEMCP_BRIDGE_PORT` and `--bridge-port` are the old names of a setting that is gone: read, said, and ignored.
OLD_BRIDGE_PORT="${MEMCP_BRIDGE_PORT:-}"
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
    --bridge-port) OLD_BRIDGE_PORT="$2"; shift ;;
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
SETTINGS="$ROOT/memcp.toml"
NODE_URL="http://127.0.0.1:$PORT"
# The connection of the assistants of this Mac. An installation made before the rename has `desktop-bridge`: it is kept.
CONNECTION=desktop
OLD_CONNECTION=desktop-bridge
SERVICE=memcp
OLD_SERVICE=memcp-capture
LABELS="org.memcp.node"
# The tasks of an earlier version: stopped and removed at every install and uninstall.
OLD_LABELS="org.memcp.bridge org.memcp.transcribe"
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
  for label in $LABELS $OLD_LABELS; do
    launchctl bootout "$DOMAIN/$label" 2>/dev/null || true
  done
}

# --- uninstall -----------------------------------------------------------------------------------------------
if [ "$ACTION" = uninstall ]; then
  step "Removing MemCP from this Mac"
  stop_agents
  for label in $LABELS $OLD_LABELS; do rm -f "$AGENTS/$label.plist"; done
  for service in $SERVICE $OLD_SERVICE; do
    for name in $CONNECTION $OLD_CONNECTION this-mac; do
      security delete-generic-password -s "$service" -a "$NODE_URL/mcp/$name" >/dev/null 2>&1 || true
    done
  done
  rm -rf "$BIN" "$SETTINGS" "$ROOT/speech.key" "$ROOT/bridge"
  say "the task, the programs and the keys are removed."
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
ask "First spaces, comma separated (a space is one box for one part of your life)" "$SPACES"; SPACES="$ANSWER"
ask "Port of the node on this Mac?" "$PORT"; PORT="$ANSWER"
case "$PORT" in ''|*[!0-9]*) die "a port must be a number" ;; esac
[ -z "$OLD_BRIDGE_PORT" ] || say "note: --bridge-port / MEMCP_BRIDGE_PORT is no longer used: the assistants reach the node on port $PORT."
NODE_URL="http://127.0.0.1:$PORT"

# --- the programs --------------------------------------------------------------------------------------------
step "2/5  Programs"
umask 077
mkdir -p "$BIN" "$DATA" "$LOGS"
chmod 700 "$ROOT" "$DATA"
PROGRAMS="memcp memcp-janitor"
if [ -n "$FROM" ]; then
  for prog in memcp-manager memcp-sync; do
    if [ -x "$FROM/$prog" ]; then PROGRAMS="$PROGRAMS $prog"; fi
  done
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
  for prog in memcp-manager memcp-sync; do
    if grep -q " $prog-$VERSION-$TARGET\$" "$TMP/SHA256SUMS"; then PROGRAMS="$PROGRAMS $prog"; fi
  done
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
for prog in memcp memcp-manager memcp-sync; do
  if [ -x "$BIN/$prog" ]; then ln -sfn "$prog" "$BIN/cartafolio${prog#memcp}"; fi
done
say "installed in $BIN"

# --- the memory ----------------------------------------------------------------------------------------------
step "3/5  Your memory"
if [ -e "$DATA/config.toml" ]; then
  say "$DATA already holds a memory: left as it is."
else
  "$BIN/memcp" --data "$DATA" init --spaces "$SPACES" --language "$LANGUAGE"
fi

# --- the node's settings -------------------------------------------------------------------------------------
# The node reads the voice notes and watches the folders itself, as memcp.toml says (docs/LINUX.md). The `bridge.conf`
# an earlier version wrote held the voice settings: they pass into memcp.toml once, and its folder is kept aside.
OLD_CONF="$ROOT/bridge/bridge.conf"
OLD_SPEECH_KEY=
if [ -f "$OLD_CONF" ] && [ ! -e "$SETTINGS" ]; then
  conf_value() { sed -n "s/^$1 = //p" "$OLD_CONF" | head -1; }
  [ -n "$WHISPER_MODEL" ] || WHISPER_MODEL=$(conf_value whisper_model)
  [ -n "$SPEECH_API" ] || SPEECH_API=$(conf_value whisper_api_url)
  OLD_SPEECH_KEY=$(conf_value whisper_api_key)
  say "the voice settings of the earlier bridge.conf are carried over into $SETTINGS"
fi
if [ -d "$ROOT/bridge" ]; then
  mv "$ROOT/bridge" "$ROOT/bridge.migrated"
  say "the folder of the earlier version is kept as $ROOT/bridge.migrated (what it held is in the node now); delete it when you like."
fi
if [ ! -e "$SETTINGS" ] || [ -n "$WHISPER_MODEL$SPEECH_API" ]; then
  {
    echo "# The node's settings, written by install-mac.sh. Every option: \`memcp config example\`."
    # What the person added by hand stays; the voice lines are written again.
    if [ -f "$SETTINGS" ]; then grep -v '^transcribe' "$SETTINGS" || true; fi
    if [ -n "$WHISPER_MODEL" ] || [ -n "$SPEECH_API" ]; then
      echo "transcribe = true"
      echo "transcribe_language = \"$LANGUAGE\""
    fi
    if [ -n "$WHISPER_MODEL" ]; then
      [ -f "$WHISPER_MODEL" ] || die "no model at $WHISPER_MODEL"
      echo "transcribe_model = \"$WHISPER_MODEL\""
      for c in whisper-cli ffmpeg; do
        p=$(command -v "$c" || true)
        [ -n "$p" ] || die "$c is needed for --whisper-model: brew install whisper-cpp ffmpeg"
        [ "$c" = whisper-cli ] && echo "transcribe_cmd = \"$p\"" || echo "transcribe_ffmpeg = \"$p\""
      done
    elif [ -n "$SPEECH_API" ]; then
      echo "transcribe_api_url = \"$SPEECH_API\""
      SPEECH_KEY="${MEMCP_SPEECH_API_KEY:-$OLD_SPEECH_KEY}"
      if [ -n "$SPEECH_KEY" ]; then
        printf '%s' "$SPEECH_KEY" > "$ROOT/speech.key"
        chmod 600 "$ROOT/speech.key"
        echo "transcribe_api_key_file = \"$ROOT/speech.key\""
      fi
    fi
  } > "$SETTINGS.new"
  mv "$SETTINGS.new" "$SETTINGS"
fi
chmod 600 "$SETTINGS"

# --- the node, running ---------------------------------------------------------------------------------------
step "4/5  The node and its keys"
plist() { # label, program arguments...
  label=$1; shift
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
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
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
for label in $OLD_LABELS; do rm -f "$AGENTS/$label.plist"; done
# Another program on the node's port would answer in its place.
if curl -fs -o /dev/null "$NODE_URL/health" 2>/dev/null || lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  die "port $PORT is taken by another program (lsof -nP -iTCP:$PORT); choose another with --port"
fi
mkdir -p "$AGENTS"
plist org.memcp.node "$BIN/memcp" --data "$DATA" --config "$SETTINGS" serve --listen "127.0.0.1:$PORT"
# The node runs from here on: as its task, or for the time of the install only (--no-agents).
NODE_PID=
if [ "$AGENTS_ON" = 1 ]; then
  launchctl bootstrap "$DOMAIN" "$AGENTS/org.memcp.node.plist"
else
  "$BIN/memcp" --data "$DATA" --config "$SETTINGS" serve --listen "127.0.0.1:$PORT" >>"$LOGS/node.log" 2>&1 &
  NODE_PID=$!
fi
i=0
until curl -fs "$NODE_URL/health" >/dev/null 2>&1; do
  i=$((i + 1)); [ $i -lt 30 ] || die "the node does not answer on $NODE_URL: see $LOGS/node.log"
  sleep 1
done

CONSOLE=$("$BIN/memcp" --data "$DATA" console-token owner 2>/dev/null | grep -o 'hcpc_[A-Za-z0-9_]*' | head -1 || true)
[ -n "$CONSOLE" ] || die "no console key could be made"

keychain_has() { security find-generic-password -s "$1" -a "$2" >/dev/null 2>&1; }
# A key kept by an earlier version under its old service name moves to the new one.
keychain_move() {
  if keychain_has "$OLD_SERVICE" "$1" && ! keychain_has "$SERVICE" "$1"; then
    v=$(security find-generic-password -s "$OLD_SERVICE" -a "$1" -w 2>/dev/null || true)
    if [ -n "$v" ] && security add-generic-password -U -s "$SERVICE" -a "$1" -l "MemCP" -w "$v"; then
      security delete-generic-password -s "$OLD_SERVICE" -a "$1" >/dev/null 2>&1 || true
    fi
    v=
  fi
}
for name in $CONNECTION $OLD_CONNECTION this-mac; do keychain_move "$NODE_URL/mcp/$name"; done
# This Mac joins its own memory as a device, as a phone does: that makes the person's Inbox, where every capture
# goes. The device's key (it writes only to the Inbox, reads nothing) is kept in the Keychain under its address.
DEVICE_ACCOUNT="$NODE_URL/mcp/this-mac"
if ! keychain_has "$SERVICE" "$DEVICE_ACCOUNT"; then
  CODE=$(curl -fsS -X POST -H "Authorization: Bearer $CONSOLE" -H "X-Requested-With: memcp" \
    -H "Content-Type: application/json" -d '{}' "$NODE_URL/admin/api/pairing" |
    sed -n 's/.*"code"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  [ -n "$CODE" ] || die "the node gave no pairing code"
  PAIRED=$(curl -fsS -X POST -H "Content-Type: application/json" \
    -d "{\"code\":\"$CODE\",\"device\":\"this-mac\"}" "$NODE_URL/pair") || die "the pairing of this Mac failed"
  DEVICE_KEY=$(printf '%s' "$PAIRED" | grep -o 'hcp_[A-Za-z0-9_-]*' | head -1)
  [ -n "$DEVICE_KEY" ] || die "no key in the pairing's answer"
  security add-generic-password -U -s "$SERVICE" -a "$DEVICE_ACCOUNT" -l "MemCP, this Mac" -w "$DEVICE_KEY"
  DEVICE_KEY=
  say "this Mac is paired with its memory: its captures go to the Inbox."
fi

# The connection of the assistants on this Mac. A connection made by an earlier version keeps its name.
if keychain_has "$SERVICE" "$NODE_URL/mcp/$OLD_CONNECTION"; then CONNECTION=$OLD_CONNECTION; fi
CONNECTION_URL="$NODE_URL/mcp/$CONNECTION"
if keychain_has "$SERVICE" "$CONNECTION_URL"; then
  say "the assistants' connection already has its key in the Keychain."
else
  OUT=$("$BIN/memcp" --data "$DATA" connect "$CONNECTION" --all --public-url "$NODE_URL" 2>&1 ||
        "$BIN/memcp" --data "$DATA" connect "$CONNECTION" --all --rotate --public-url "$NODE_URL" 2>&1) ||
    die "no key could be made for the assistants' connection: $OUT"
  KEY=$(printf '%s\n' "$OUT" | grep -o 'hcp_[A-Za-z0-9_-]*' | head -1)
  [ -n "$KEY" ] || die "no key in the node's answer"
  security add-generic-password -U -s "$SERVICE" -a "$CONNECTION_URL" -l "MemCP, assistants of this Mac" -w "$KEY"
  KEY=
  say "a key for the assistants of this Mac is in the Keychain (service $SERVICE)."
fi

# --- run it --------------------------------------------------------------------------------------------------
step "5/5  Run it"
if [ "$AGENTS_ON" = 1 ]; then
  say "the node answers on $NODE_URL; it starts again at login."
else
  [ -z "$NODE_PID" ] || kill "$NODE_PID" 2>/dev/null || true
  say "nothing left running (--no-agents). Start it with:"
  say "  launchctl bootstrap $DOMAIN $AGENTS/org.memcp.node.plist"
fi

TOKEN=$CONSOLE
cat <<EOF

The page of your memory, on this Mac only:
  $NODE_URL/admin
EOF
[ -z "$TOKEN" ] || printf 'Its console key (shown once; it opens the page, never the memory):\n  %s\n' "$TOKEN"
cat <<EOF

An assistant of this Mac reaches the memory at $CONNECTION_URL. Its key is in the Keychain; read it only when you
configure the assistant:
  security find-generic-password -s $SERVICE -a "$CONNECTION_URL" -w
For Claude Code:
  claude mcp add --transport http memcp $CONNECTION_URL --header "Authorization: Bearer \$(security find-generic-password -s $SERVICE -a $CONNECTION_URL -w)"

Voice notes and watched folders are set in $SETTINGS (\`memcp config example\` lists the options);
restart the node after a change: launchctl kickstart -k $DOMAIN/org.memcp.node
Logs:     $LOGS
Update:   run this installer again; your memory is left alone.
Remove:   sh install-mac.sh --uninstall   (the memory is kept; add --purge to delete it)
EOF
