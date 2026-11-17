#!/usr/bin/with-contenv bashio
# Starts memcp-manager inside Home Assistant: one memory per person, each in a directory of
# its own under /data/manager (part of the add-on backups), the admin page through ingress, MCP
# on port 8791.
set -e
DATA=/data/manager
MANAGER=/usr/bin/memcp-manager
LANGUAGE=$(bashio::config 'language')

# Coming from another installation (a former HemCP or MemCP app): the folders it left in this app's own configuration
# folder (`/config/import/manager`, and `/config/import/janitor` if it had a janitor) are taken in once, when this app
# has no data of its own yet. The import folder is then renamed, never deleted. See DOCS.md, "Moving from another app".
IMPORT=/config/import
if [ ! -d "$DATA/registry" ] && [ -d "$IMPORT/manager/registry" ]; then
  bashio::log.info "importing the memory left in $IMPORT/manager"
  mkdir -p "$DATA"
  cp -a "$IMPORT/manager/." "$DATA/"
  if [ -d "$IMPORT/janitor" ]; then mkdir -p /data/janitor && cp -a "$IMPORT/janitor/." /data/janitor/; fi
  mv "$IMPORT" "/config/import.done-$(date +%Y%m%d-%H%M%S)"
  bashio::log.info "imported; the folder was renamed import.done-… and can be deleted once everything checks out"
fi

ARGS=(--data "$DATA" --language "$LANGUAGE" serve --listen 0.0.0.0:8791 --allow-remote
      --ingress-listen 0.0.0.0:8099 --trust-ingress 172.30.32.2)
# Found without typing an address: the node advertises itself on the local network (host network, see DOCS.md).
NODE_NAME="MemCP"
if bashio::config.has_value 'node_name'; then NODE_NAME="$(bashio::config 'node_name')"; fi
# Advertising needs the host network, which is not set (its ports clash with other apps): the name is kept for it.
ARGS+=(--node-name "$NODE_NAME")
ADMINS=$(bashio::config 'admins' | paste -sd, -)
# Every Home Assistant user gets a memory of their own unless asked otherwise.
if bashio::config.has_value 'new_people'; then ARGS+=(--new-people "$(bashio::config 'new_people')"); fi
[ -n "$ADMINS" ] && ARGS+=(--admins "$ADMINS")
if bashio::config.has_value 'public_url'; then
  ARGS+=(--public-url "$(bashio::config 'public_url')")
else
  # No Supervisor API access needed: the usual local name of Home Assistant.
  ARGS+=(--public-url "http://homeassistant.local:8791")
fi
# The OAuth consent page and the admin page for the tailnet. The server believes the login
# header only from the one address in identity_trust (the machine running Tailscale serve).
if bashio::config.has_value 'identity_trust'; then
  ARGS+=(--identity-listen 0.0.0.0:8792 --identity-trust "$(bashio::config 'identity_trust')")
  bashio::config.has_value 'login_url' && ARGS+=(--login-url "$(bashio::config 'login_url')")
  TAILNET=$(bashio::config 'tailnet_users' | paste -sd, -)
  [ -n "$TAILNET" ] && ARGS+=(--tailnet-user "$TAILNET")
fi
# The janitor, optional: once the memory answers, it gets a key of its own (made once, kept in /data/janitor) and files the
# notes of the inboxes with the model named here (an API, or an Ollama on the network): once the writing into an inbox has
# calmed down (janitor_quiet seconds after the last write), when an assistant asks (`tidy`), and once a night (janitor_night).
# Never for nothing.
if bashio::config.true 'janitor'; then
  PERSON=$(bashio::config 'janitor_person')
  JDIR=/data/janitor
  if [ -z "$PERSON" ]; then
    bashio::log.warning "janitor: janitor_person is needed; it is not started"
  elif ! bashio::config.has_value 'janitor_api_url' && ! bashio::config.has_value 'janitor_ollama_url'; then
    bashio::log.warning "janitor: no model (janitor_api_url or janitor_ollama_url); it is not started"
  else
    (
      umask 077
      mkdir -p "$JDIR"
      until curl -sf http://127.0.0.1:8791/health >/dev/null; do sleep 2; done
      # A key that the memory refuses (the connection was revoked) is made again, never kept.
      if [ -s "$JDIR/key" ]; then
        CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $(cat "$JDIR/key")" \
               -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"ping"}' \
               http://127.0.0.1:8791/mcp/janitor || echo 000)
        if [ "$CODE" = "401" ] || [ "$CODE" = "403" ]; then
          bashio::log.warning "janitor: its key is refused (revoked?): making a new one"
          rm -f "$JDIR/key"
        fi
      fi
      if [ ! -s "$JDIR/key" ]; then
        OUT=$(/usr/bin/memcp --data "$DATA/people/$PERSON" connect janitor --all --internal 2>&1 ||
              /usr/bin/memcp --data "$DATA/people/$PERSON" connect janitor --all --rotate --internal 2>&1 || true)
        echo "$OUT" | grep -oE 'hcp_[A-Za-z0-9_-]+' | head -1 | tr -d '\n' > "$JDIR/key"
      fi
      # Also for a key made before connections had a kind: the janitor is an internal tool, not an assistant.
      /usr/bin/memcp --data "$DATA/people/$PERSON" internal janitor on >/dev/null 2>&1 || true
      if [ ! -s "$JDIR/key" ]; then
        bashio::log.error "janitor: no key could be made for $PERSON; it is not started"
        exit 0
      fi
      MODELS=""
      CONF="$JDIR/janitor.conf"
      {
        echo "url = http://127.0.0.1:8791/mcp/janitor"
        echo "key_file = $JDIR/key"
        echo "state_dir = $JDIR/state"
        INBOXES=$(bashio::config 'janitor_inboxes' | paste -sd, -)
        [ -n "$INBOXES" ] && echo "inbox_vaults = $INBOXES"
        WORKWORDS=$(bashio::config 'janitor_work_words' | paste -sd, -)
        [ -n "$WORKWORDS" ] && echo "work_words = $WORKWORDS"
        if bashio::config.has_value 'janitor_api_url'; then
          printf '%s' "$(bashio::config 'janitor_api_key')" > "$JDIR/api.key"
          echo "api_url = $(bashio::config 'janitor_api_url')"
          echo "api_model = $(bashio::config 'janitor_api_model')"
          echo "api_key_file = $JDIR/api.key"
          MODELS="api"
        fi
        if bashio::config.has_value 'janitor_ollama_url'; then
          echo "ollama_url = $(bashio::config 'janitor_ollama_url')"
          echo "ollama_model = $(bashio::config 'janitor_ollama_model')"
          MODELS="${MODELS:+$MODELS,}local"
        fi
        echo "models = $MODELS"
      } > "$CONF"
      bashio::log.info "janitor: filing the inboxes of $PERSON after writing, on request, and at $(bashio::config 'janitor_night') ($MODELS)"
      exec /usr/bin/memcp-janitor --config "$CONF" --watch "$DATA/people/$PERSON/inbox-written" \
        --quiet "$(bashio::config 'janitor_quiet')" --night "$(bashio::config 'janitor_night')"
    ) &
  fi
fi

# Voice notes, optional: a recording kept in a space (recorded on the page, sent by a phone) waits under `inbox/audio`.
# With a speech service named here (an API of the OpenAI kind: `…/v1/audio/transcriptions`), the bridge reads each
# recording, puts its text in the same space, and erases the recording a week later. The recording LEAVES this machine
# for that service: turn it on only for a service you trust. Computers that run their own bridge with a local speech
# model do the same without any service.
if bashio::config.true 'transcribe'; then
  TPERSON=$(bashio::config 'transcribe_person')
  [ -z "$TPERSON" ] && TPERSON=$(bashio::config 'janitor_person')
  TDIR=/data/transcribe
  if [ -z "$TPERSON" ]; then
    bashio::log.warning "transcribe: transcribe_person is needed; it is not started"
  elif ! bashio::config.has_value 'transcribe_api_url'; then
    bashio::log.warning "transcribe: no speech service (transcribe_api_url); it is not started"
  else
    (
      umask 077
      mkdir -p "$TDIR"
      until curl -sf http://127.0.0.1:8791/health >/dev/null; do sleep 2; done
      if [ -s "$TDIR/key" ]; then
        CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $(cat "$TDIR/key")" \
               -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"ping"}' \
               http://127.0.0.1:8791/mcp/haos-voice || echo 000)
        if [ "$CODE" = "401" ] || [ "$CODE" = "403" ]; then rm -f "$TDIR/key"; fi
      fi
      if [ ! -s "$TDIR/key" ]; then
        OUT=$(/usr/bin/memcp --data "$DATA/people/$TPERSON" connect haos-voice --all 2>&1 ||
              /usr/bin/memcp --data "$DATA/people/$TPERSON" connect haos-voice --all --rotate 2>&1 || true)
        echo "$OUT" | grep -oE 'hcp_[A-Za-z0-9_-]+' | head -1 | tr -d '\n' > "$TDIR/key"
      fi
      if [ ! -s "$TDIR/key" ]; then
        bashio::log.error "transcribe: no key could be made for $TPERSON; it is not started"
        exit 0
      fi
      SPACES=$(bashio::config 'transcribe_spaces' | tr -d ' ')
      [ -z "$SPACES" ] && SPACES="${TPERSON}-a-trier"
      CONF="$TDIR/bridge.conf"
      {
        echo "url = http://127.0.0.1:8791/mcp/haos-voice"
        echo "key_file = $TDIR/key"
        echo "outbox = $TDIR/outbox"
        echo "transcribe = $SPACES"
        echo "whisper_api_url = $(bashio::config 'transcribe_api_url')"
        echo "whisper_api_model = $(bashio::config 'transcribe_api_model')"
        if bashio::config.has_value 'transcribe_api_key'; then
          echo "whisper_api_key = $(bashio::config 'transcribe_api_key')"
        fi
        if bashio::config.has_value 'transcribe_language'; then
          echo "whisper_lang = $(bashio::config 'transcribe_language')"
        fi
      } > "$CONF"
      chmod 600 "$CONF"
      bashio::log.info "transcribe: reading the recordings of $SPACES every minute, by the speech service"
      while true; do
        /usr/bin/memcp-capture transcribe --config "$CONF" || true
        sleep 60
      done
    ) &
  fi
fi

bashio::log.info "memcp-manager starting"
exec "$MANAGER" "${ARGS[@]}"
