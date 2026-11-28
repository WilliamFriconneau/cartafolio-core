#!/bin/sh
set -eu

OPTIONS=/data/options.json
CONFIG=/data/pensine.toml

allow_remote=$(jq -r '.allow_remote // false' "$OPTIONS")
public_url=$(jq -r '.public_url // ""' "$OPTIONS")

if [ "$allow_remote" = "true" ]; then
    listen="0.0.0.0:8791"
else
    listen="127.0.0.1:8791"
fi

{
    echo "listen = \"$listen\""
    echo "allow_remote = $allow_remote"
    if [ -n "$public_url" ]; then
        printf 'public_url = "%s"\n' "$public_url"
    fi
    echo 'admin_listen = "0.0.0.0:8099"'
    echo 'admin_trusted_proxy = "172.30.32.2"'
    echo 'data_dir = "/data"'
} > "$CONFIG"

exec pensine serve --data /data --config "$CONFIG"
