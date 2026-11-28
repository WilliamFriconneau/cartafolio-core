#!/usr/bin/with-contenv bashio
set -eu

config=/data/pensine.toml
address_file=/data/public_url
rm -f "$address_file"

start_funnel() {
    for attempt in $(seq 1 30); do
        if [ -S "$socket" ]; then break; fi
        sleep 1
    done
    if [ ! -S "$socket" ]; then
        bashio::log.error "Tailscale did not start."
        return 1
    fi

    hostname=$(bashio::config 'tailscale_hostname')
    set -- "--hostname=${hostname:-pensine}" --accept-dns=false --accept-routes=false
    if bashio::config.has_value 'tailscale_authkey'; then
        set -- "$@" "--authkey=$(bashio::config 'tailscale_authkey')"
    fi
    bashio::log.info "Tailscale is signing in. If an approval URL appears below, open it once."
    if ! tailscale --socket="$socket" up "$@"; then
        bashio::log.error "Tailscale sign-in failed."
        return 1
    fi

    if ! status=$(tailscale --socket="$socket" status --json) ||
        ! dns=$(printf '%s' "$status" | jq -er '.Self.DNSName | strings | rtrimstr(".") | select(length > 0)'); then
        bashio::log.error "Tailscale did not provide a DNS name."
        return 1
    fi
    if ! tailscale --socket="$socket" funnel --bg --https=443 http://127.0.0.1:8791; then
        bashio::log.error "Funnel could not start. Enable Funnel for this node and restart the add-on."
        return 1
    fi
    printf 'https://%s\n' "$dns" > "$address_file.tmp"
    mv "$address_file.tmp" "$address_file"
    bashio::log.info "Pensine public address: https://$dns"
}

allow_plain_http=false
if bashio::config.true 'tailscale'; then
    socket=/var/run/tailscale/tailscaled.sock
    mkdir -p /data/tailscale /var/run/tailscale
    tailscaled --tun=userspace-networking --state=/data/tailscale/tailscaled.state --socket="$socket" >/dev/null 2>&1 &
    start_funnel &
    listen=127.0.0.1:8791
    allow_remote=false
    public_url=
else
    listen=0.0.0.0:8791
    allow_remote=true
    exposed_port=$(bashio::addon.port '8791/tcp' || true)
    if [ -n "$exposed_port" ] && [ "$exposed_port" != null ]; then
        if bashio::config.true 'allow_plain_http'; then
            allow_plain_http=true
            bashio::log.warning "Port 8791 serves plain HTTP. Use a TLS proxy to protect credentials in transit."
        else
            bashio::log.warning "Port 8791 is mapped but Pensine listens only on loopback. Enable allow_plain_http to opt in."
        fi
    fi
    public_url=
    if bashio::config.has_value 'public_url'; then
        public_url=$(bashio::config 'public_url')
    fi
fi

{
    printf 'listen = "%s"\n' "$listen"
    printf 'allow_plain_http = %s\n' "$allow_plain_http"
    printf 'allow_remote = %s\n' "$allow_remote"
    printf 'public_url_file = "%s"\n' "$address_file"
    if [ -n "$public_url" ]; then
        printf 'public_url = %s\n' "$(jq -n --arg url "$public_url" '$url')"
    fi
    echo 'admin_listen = "0.0.0.0:8099"'
    echo 'admin_trusted_proxy = "172.30.32.2"'
    echo 'data_dir = "/data"'
} > "$config"

exec /usr/bin/pensine serve --data /data --config "$config"
