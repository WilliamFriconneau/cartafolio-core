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
# With no option, the manager uses the stored name or the default for a new node.
if bashio::config.has_value 'node_name'; then ARGS+=(--node-name "$(bashio::config 'node_name')"); fi
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

# The node's own intake, optional: the voice notes and the watched folders are tasks of the node itself, not of another
# program. The options are written once into a settings file (memcp.toml format) that the node reads.
#  - transcribe: a recording kept in a space (recorded on the page, sent by a phone) waits under `inbox/audio`. With a
#    speech service named here (an API of the OpenAI kind: `.../v1/audio/transcriptions`), the node reads each recording
#    and puts its text in the same space. The recording LEAVES this machine for that service: turn it on only for a
#    service you trust.
#  - watch: a file added to a folder (`/share/...`, `/media/...`) becomes a note or a file of a space. The files stay.
INTAKE_CONF=/data/memcp.toml
{
  umask 077
  : > "$INTAKE_CONF"
  PERSON_NAMED=$(bashio::config 'intake_person')
  [ -z "$PERSON_NAMED" ] && PERSON_NAMED=$(bashio::config 'transcribe_person')
  [ -z "$PERSON_NAMED" ] && PERSON_NAMED=$(bashio::config 'janitor_person')
  if [ -n "$PERSON_NAMED" ]; then echo "intake_person = \"$PERSON_NAMED\"" >> "$INTAKE_CONF"; fi
  if bashio::config.true 'transcribe'; then
    if ! bashio::config.has_value 'transcribe_api_url'; then
      bashio::log.warning "transcribe: no speech service (transcribe_api_url); voice notes are not read"
    else
      {
        echo "transcribe = true"
        echo "transcribe_api_url = \"$(bashio::config 'transcribe_api_url')\""
        echo "transcribe_api_model = \"$(bashio::config 'transcribe_api_model')\""
        SPACES=$(bashio::config 'transcribe_spaces' | tr -d ' ')
        # Empty means every space the person writes to, those made later included.
        echo "transcribe_spaces = \"${SPACES:-all}\""
        if bashio::config.has_value 'transcribe_api_key'; then
          printf '%s' "$(bashio::config 'transcribe_api_key')" > /data/transcribe.key
          echo "transcribe_api_key_file = \"/data/transcribe.key\""
        fi
        if bashio::config.has_value 'transcribe_language'; then
          echo "transcribe_language = \"$(bashio::config 'transcribe_language')\""
        fi
        # Guided transcription: on unless the option is turned off.
        if bashio::config.false 'transcribe_hints'; then
          echo "transcribe_hints = false"
        else
          echo "transcribe_hints = true"
        fi
      } >> "$INTAKE_CONF"
      bashio::log.info "transcribe: the node reads the voice notes by the speech service"
    fi
  fi
  WATCH=$(bashio::config 'watch' | sed 's/"/\\"/g; s/.*/"&"/' | paste -sd, -)
  if [ -n "$WATCH" ]; then
    echo "watch = [$WATCH]" >> "$INTAKE_CONF"
    bashio::log.info "watch: the node takes what is added to $(bashio::config 'watch' | paste -sd' ' -)"
  fi
}
ARGS+=(--config "$INTAKE_CONF")

bashio::log.info "memcp-manager starting"
exec "$MANAGER" "${ARGS[@]}"
