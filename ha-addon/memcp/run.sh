#!/usr/bin/with-contenv bashio
# Starts memcp-manager inside Home Assistant: one memory per person, each in a directory of
# its own under /data/manager (part of the add-on backups), the admin page through ingress, MCP
# on port 8791.
set -e
DATA=/data/manager
MANAGER=/usr/bin/memcp-manager
LANGUAGE=$(bashio::config 'language')

ARGS=(--data "$DATA" --language "$LANGUAGE" serve --listen 0.0.0.0:8791 --allow-remote
      --ingress-listen 0.0.0.0:8099 --trust-ingress 172.30.32.2)
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
      if [ ! -s "$JDIR/key" ]; then
        OUT=$(/usr/bin/memcp --data "$DATA/people/$PERSON" connect janitor --all 2>&1 ||
              /usr/bin/memcp --data "$DATA/people/$PERSON" connect janitor --all --rotate 2>&1 || true)
        echo "$OUT" | grep -oE 'hcp_[A-Za-z0-9_-]+' | head -1 | tr -d '\n' > "$JDIR/key"
      fi
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
        /usr/bin/memcp-bridge transcribe --config "$CONF" || true
        sleep 60
      done
    ) &
  fi
fi

bashio::log.info "memcp-manager starting"
exec "$MANAGER" "${ARGS[@]}"
