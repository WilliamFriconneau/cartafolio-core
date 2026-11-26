# Pensine on Linux

The same complete node as on Home Assistant, as a system service of your own machine: the memory, its page, its janitor, its voice. Settings are in one file, `memcp.toml` (every option is listed in the file itself and matched with the add-on's in [PARITY.md](PARITY.md)).

## Install

Pick one.

**A package.** Debian and Ubuntu: `packaging/linux/build-deb.sh` builds `target/debian/memcp_*.deb` (`sudo apt install ./memcp_*.deb`). Arch and CachyOS: `makepkg -si` in `packaging/linux/` with the `PKGBUILD`. Both install `memcp` and `pensine-janitor` in `/usr/bin`, the units, the `memcp` user, and `/etc/memcp/memcp.toml`.

**One command.** `curl -fsSL https://raw.githubusercontent.com/WilliamFriconneau/pensine-core/main/install.sh | sh` installs for your own user (programs in `~/.local/bin`, memory in `~/memory`, a user service). `sudo sh install.sh --system` does the package's job from the release archive: it checks the download against `SHA256SUMS`, makes the user `memcp`, `/var/lib/memcp`, `/etc/memcp/memcp.toml` and the units. `--help` lists the options. The archive (`memcp-<version>-<target>.tar.gz`, built by `packaging/linux/build-tarball.sh`) also carries `install.sh` and installs from itself, offline.

**From source.** `cargo install --path crates/server` and `cargo install --path crates/janitor`.

To check installer path quoting on a Linux machine with a systemd user session, run this from an unpacked release archive:

```sh
check_dir=$(mktemp -d "$HOME/pensine-install-check.XXXXXX")
XDG_CONFIG_HOME="$check_dir/config files" MEMCP_SERVICE=yes \
  sh ./install.sh --yes --port 18791 --prefix "$check_dir/bin & tools" --data "$check_dir/my memory"
systemctl --user status memcp
curl -fsS http://127.0.0.1:18791/health
"$check_dir/bin & tools/pensine" --data "$check_dir/my memory" doctor
```

Use a test account without an existing Pensine service. If the user manager does not load the temporary configuration directory, run `systemd-analyze --user verify "$check_dir/config files/systemd/user/memcp.service"` and test the generated unit from the user manager's regular unit directory.

## First run

```sh
sudo memcp setup --system        # as a service; as yourself, without sudo and --system, for a memory of your own
sudo systemctl enable --now memcp memcp-backup.timer
memcp doctor
```

`memcp setup` asks a few questions on a terminal (where the memory lives, the name of this node, the language, the listen address, the public address, your name, the first spaces) and takes the answer you give or the default. With `--yes`, or when there is no terminal, it asks nothing:

```sh
memcp setup --system --yes --person "Ana" --node-name atelier --language fr \
  --public-url https://memory.example.net --spaces personal,health,home
```

It makes the data folder (mode `0700`, its files `0600`, owned by the service user), the first memory with its spaces, `memcp.toml` (only when there is none; `--force` writes it again), and then prints two things, once:

- the address of **your page**, on this machine only (`http://127.0.0.1:8791/admin`), and a **console key** that opens it;
- **one address for an assistant that takes a key, with the key in it** (`https://.../mcp/assistant?key=...`), and the same as a `claude mcp add` line. Paste it in the assistant and it is connected. The key reaches the spaces that were made and nothing else; shrink it later with `memcp regrant assistant --grant health:rw`.

Run again, `setup` leaves an existing memory and settings alone.

## The page: signing in

The node page (`/admin`) is the same page as in Home Assistant, outside Home Assistant. Two ways in, both private:

1. **Console key.** On the machine, `memcp console-token owner` (or the one `setup` printed) gives a key that starts with `hcpc_`. Open `http://127.0.0.1:8791/admin` and paste it. It opens the page and never the memory over MCP; an assistant's key does not open the page. The page is served on the listen address while that is the loopback (`console_admin = true` serves it on another one), never through Tailscale Funnel; wrong keys are counted and then blocked. To reach it from another computer without opening a port, use an SSH tunnel: `ssh -L 8791:127.0.0.1:8791 that-machine`.
2. **Tailnet identity.** With Tailscale, the page knows who you are. In `memcp.toml`:

   ```toml
   identity_listen = "127.0.0.1:8792"
   login_url = "https://memory.example.ts.net:8443"
   tailnet_users = ["you@example.org=owner"]
   ```

   and `sudo tailscale serve --bg --https=8443 http://127.0.0.1:8792`. The identity listener believes the `Tailscale-User-Login` header only from the loopback (or from the one address in `identity_trust`), so any other caller gets 403; a login that is not in `tailnet_users` is refused. Keep this listener private. Assistants that sign in open the consent page on the public HTTPS address and use a one-time invitation code from **Connect an assistant**. Devices use their own pairing keys.

Both are checked by tests: `crates/server/tests/admin.rs` (console keys open the page, MCP keys do not, the page is not served through Funnel, failed keys are blocked), `crates/server/tests/oauth.rs` (the page on the tailnet knows the owner, a stranger is refused) and `crates/server/tests/linux.rs` (the same through the real program, started from `memcp.toml`).

## Settings

The search starts with `--config FILE`, then the first nonempty value of `PENSINE_CONFIG`, `SILICE_CONFIG`, and `MEMCP_CONFIG`. Without an explicit path, it checks `~/.config/pensine`, `/etc/pensine`, `~/.config/memcp`, `~/.config/silice`, `/etc/memcp`, and `/etc/silice` in that order. An absolute `XDG_CONFIG_HOME` replaces `~/.config`. The final fallback is the directory beside the data directory supplied by flag or environment. In each directory, it looks for `pensine.toml`, then `silice.toml`, then `memcp.toml`. An explicit path is used as given.

The file uses plain `key = value`; `memcp config check` reads it and says what is wrong, `memcp config example` prints the documented file, `memcp config show` prints yours with every option. **A flag on the command line wins over the file.** After a change: `sudo systemctl restart memcp`.

Memory itself is never in the file: no key, no password. A service key (the janitor's model, a speech service) is a file whose path you give (`janitor_api_key_file`).

### Companions

- **Read before write.** Set `strict_write_agents = ["my-assistant"]` to require a recent read before selected MCP writes. A single `"*"` selects every connection except apps (names starting with `app-` or ending with `-app`), peers, consoles and the janitor. The default is `[]`. The first nonempty comma-separated value from `PENSINE_STRICT_WRITE_AGENTS`, `SILICE_STRICT_WRITE_AGENTS`, or `MEMCP_STRICT_WRITE_AGENTS` overrides the file. The `--strict-write-agents` flag takes precedence.
- **Janitor.** `janitor = true`, a person, and a model (`janitor_ollama_url` or `janitor_api_url`), then `sudo systemctl enable --now pensine-janitor`. The unit runs `memcp prepare janitor` first, which makes the janitor's own key and its settings from `memcp.toml`. See [JANITOR.md](JANITOR.md).
- **Voice notes.** `transcribe = true` with `transcribe_api_url` (a speech service: the recording leaves this machine for it) or `transcribe_model` (whisper.cpp and `ffmpeg` on this machine). The node reads the recordings itself while it runs, with no other unit; `memcp transcribe` is one pass by hand. Folders to watch: `watch = ["/srv/dictation -> personal"]`. See [INTAKE.md](INTAKE.md). A system that had `memcp-transcribe.timer` can disable it (`sudo systemctl disable --now memcp-transcribe.timer`): the package no longer ships it.
- **Backups.** `memcp-backup.timer` runs `memcp backup` every night: a consistent copy of every database and of the files in `backup_dir` (`/var/backups/memcp`), the last `backup_keep` (14) kept. The same by hand: `memcp backup --to DIR --keep 14`.

## Upgrade

Take a backup, install the new version, restart.

```sh
sudo memcp backup && memcp doctor     # before: nothing wrong, and a copy
sudo apt install ./memcp_*.deb        # or: pacman -U ..., or sudo sh install.sh --system
sudo systemctl restart memcp
memcp doctor
```

The databases are brought up to date by themselves the first time the new version opens them, and an older version cannot open them afterwards: that is why the backup comes first. `memcp doctor` says if a database is older (it migrates at the next start) or newer than the program. Your `memcp.toml` is a configuration file of the package: it is never replaced without asking (Debian asks, Arch leaves a `.pacnew`).

## Backup and restore

`memcp backup` makes `backup_dir/backup-<date>/` (each database copied with SQLite's own copy and checked, the stored files, the settings of the memory), safe while the node runs. Keep a copy on another disk or machine: `rsync -a /var/backups/memcp/ other:/backups/memcp/`.

To restore, with the service stopped:

```sh
sudo systemctl stop memcp
sudo mv /var/lib/memcp /var/lib/memcp.broken            # keep it until the restore is proven
sudo cp -a /var/backups/memcp/backup-YYYYMMDD-HHMMSS /var/lib/memcp
sudo chown -R memcp:memcp /var/lib/memcp && sudo chmod 700 /var/lib/memcp
sudo -u memcp memcp doctor
sudo systemctl start memcp
```

Connections and their keys are in the backup (only their digests), so every assistant keeps working. To move to another machine: the same, on the new one after install.

## Uninstall

```sh
sudo systemctl disable --now memcp memcp-backup.timer pensine-janitor
sudo apt remove memcp        # or: sudo pacman -R memcp
```

The package never removes your memory, your backups or `memcp.toml`, even when purged. To erase them for good: `sudo rm -r /var/lib/memcp /var/backups/memcp /etc/memcp /var/lib/pensine-janitor /var/lib/memcp-transcribe`, and `sudo userdel memcp`. Export what you want to keep first: `memcp export --to DIR`.

## Firewall and network

The node listens on the loopback (`127.0.0.1:8791`) and nothing else, so with the defaults **no port is open** and no rule is needed. Keys travel in clear on that port: an assistant that is not on the machine reaches it through an HTTPS proxy on the same machine. Open only the proxy's port:

```sh
sudo ufw allow 443/tcp                                   # ufw
sudo firewall-cmd --permanent --add-service=https && sudo firewall-cmd --reload   # firewalld
```

Never open 8791 or 8792 to the network. If you must listen on another address (a container, a separate proxy machine), set `listen` and `allow_remote = true`, and let the firewall pass that one port from the proxy's address only.

## TLS

Claude on the web, and most apps, need an `https://` address (`public_url` in `memcp.toml`). Choose one.

**Tailscale serve** (private, your devices only):

```sh
sudo tailscale serve --bg http://127.0.0.1:8791        # https://<machine>.<tailnet>.ts.net
```

Funnel (`tailscale funnel`) makes it public: needed only for Claude on the web, and then only for the MCP address (see [TAILSCALE.md](TAILSCALE.md)).

**Caddy** (`/etc/caddy/Caddyfile`; certificates are automatic):

```
memory.example.net {
    reverse_proxy 127.0.0.1:8791
}
```

**nginx** (certificates from certbot):

```nginx
server {
    listen 443 ssl;
    server_name memory.example.net;
    ssl_certificate     /etc/letsencrypt/live/memory.example.net/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/memory.example.net/privkey.pem;
    client_max_body_size 32m;          # a voice note, a photo
    location / {
        proxy_pass http://127.0.0.1:8791;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_buffering off;           # streamed answers
        proxy_read_timeout 300s;
    }
}
```

Do not publish `/admin` through a public proxy: it is served on the loopback for you, and a console key is not meant to cross the internet. Block it at the proxy if you like (`location /admin { return 404; }`). Then set `public_url = "https://memory.example.net"` and check with `memcp doctor`.

## What `memcp doctor` checks

`memcp doctor` reads the installation and writes nothing; it is safe on a live node and before an upgrade. `ok` is fine, `info` is for your eyes, `warn` deserves a look, `FAIL` is a problem (the exit code is 1). Every check below exists in the program (`doctor::CHECKS`), and a test fails if this list and the code drift apart.

- `data-dir-private`: the data directory is not readable by others (fix: `chmod 700`).
- `files-private`: the database files are not readable by others (fix: `chmod 600`).
- `control-version`: the control database is at the version of this program (older: it migrates at the next start; newer: do not start it).
- `control-integrity`: SQLite's quick check of the control database.
- `vault-file`: every space has its file.
- `vault-version`: every space is at the version of this program.
- `vault-integrity`: SQLite's quick check of every space.
- `inbox`: notes tagged `inbox` waiting to be filed.
- `people`: there is a person who can open the page.
- `connections`: how many connections are active and revoked.
- `connection-all`: a connection that reaches every space (prefer one limited to what it needs).
- `connection-idle`: a connection unused for 30 days.
- `connection-expired`: a connection past its date.
- `public-address`: the public address is HTTPS, has no slash at the end, and works from outside your network (with `--public-url`, or `public_url`).
- `server-answers`: something answers on the listen address like this node does (with `--listen`, or `listen`).
- `settings`: `memcp.toml` reads, and what has no effect in it is said.
- `backup`: the newest backup is recent (three days at most).

## Troubleshooting

| What you see | What to do |
|---|---|
| `systemctl status memcp` says the condition failed | The memory is not made yet: `sudo memcp setup --system`. |
| The service fails at once; `journalctl -u memcp -e` says permission denied | The data belongs to another user: `sudo chown -R memcp:memcp /var/lib/memcp`. With `data` set to another folder, add it to the unit: `sudo systemctl edit memcp` and `[Service]`, `ReadWritePaths=/that/folder`. |
| `... is not a local address: tokens would travel in clear` | `listen` is not the loopback. Put a proxy in front, or `allow_remote = true` if the firewall is yours (see Firewall). |
| `address already in use` | Another program holds 8791: change `listen`, or `ss -ltnp | grep 8791`. |
| An assistant gets 401 | The key is wrong, revoked or at another address: a key works only at its own `/mcp/<name>`. `memcp connections` lists them; `memcp connect NAME --all --rotate` makes a new key. |
| An assistant gets 403 or nothing from outside | The proxy, the firewall, or `public_url` (check with `memcp doctor --public-url https://...`). Claude on the web needs HTTPS reachable from the internet. |
| The page says the key is not valid | Console keys are shown once: `memcp console-token owner` gives a new one. A lot of wrong keys blocks the address for a while. |
| `memcp: unknown option` in `memcp.toml` | A typo: `memcp config check` names the line. |
| The janitor does nothing | `systemctl status pensine-janitor`: exit 78 means `janitor = false`, or no model named. |
| `database ... newer than this program` | You started an older version after a newer one. Install the newer one again, or restore a backup. |
| Nothing works after the machine restarted | `systemctl is-enabled memcp`; `journalctl -u memcp -b`. |

`journalctl -u memcp -f` follows the node. `memcp doctor` is the first thing to run.
