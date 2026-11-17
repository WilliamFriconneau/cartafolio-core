# Configuration parity: Home Assistant app and Linux

Every option of the Home Assistant app (`ha-addon/memcp/config.yaml`) has a counterpart on Linux, in `memcp.toml` and, where it changes how the server starts, on the command line (a flag wins over the file). The table is kept true by a test (`crates/server/tests/parity.rs`): it fails when an add-on option has no `memcp.toml` key, when this table forgets one, or when the example file `packaging/linux/memcp.toml` is out of date. The options themselves are described once, in `crates/server/src/config.rs`, which also writes the documented file (`memcp config example`).

| Add-on option | `memcp.toml` | Command line | Note |
|---|---|---|---|
| `node_name` | `node_name` | `serve --node-name` | |
| `language` | `language` | `setup --language`, `init --language` | Written once into the new memory. |
| `admins` | `admins` | `serve --admins` | Logins for the ingress listener (Home Assistant). On Linux the page is opened by console key or tailnet identity, so it is rarely needed. |
| `new_people` | `new_people` | `memcp-manager serve --new-people` | Only for `memcp-manager` (several people on one node). `memcp serve` holds one memory and ignores it. |
| `public_url` | `public_url` | `serve --public-url`, `connect --public-url`, `doctor --public-url` | |
| `identity_trust` | `identity_trust` | `serve --identity-trust` | Needs `identity_listen`. |
| `login_url` | `login_url` | `serve --login-url` | |
| `tailnet_users` | `tailnet_users` | `serve --tailnet-user` | A list in the file, `a=b,c=d` on the line. |
| `janitor` | `janitor` | `prepare janitor` | Run by `memcp-janitor.service`. |
| `janitor_person` | `janitor_person` | `prepare janitor` | |
| `janitor_inboxes` | `janitor_inboxes` | `prepare janitor` | |
| `janitor_work_words` | `janitor_work_words` | `prepare janitor` | |
| `janitor_quiet` | `janitor_quiet` | `prepare janitor` | |
| `janitor_night` | `janitor_night` | `prepare janitor` | |
| `janitor_api_url` | `janitor_api_url` | `prepare janitor` | |
| `janitor_api_model` | `janitor_api_model` | `prepare janitor` | |
| `janitor_api_key` | `janitor_api_key_file` | `prepare janitor` | The key is a file (mode 0600), never a line of `memcp.toml`. |
| `janitor_ollama_url` | `janitor_ollama_url` | `prepare janitor` | |
| `janitor_ollama_model` | `janitor_ollama_model` | `prepare janitor` | |
| `transcribe` | `transcribe` | `prepare transcribe` | Run every minute by `memcp-transcribe.timer`. |
| `transcribe_person` | `transcribe_person` | `prepare transcribe` | |
| `transcribe_spaces` | `transcribe_spaces` | `prepare transcribe` | |
| `transcribe_api_url` | `transcribe_api_url` | `prepare transcribe` | |
| `transcribe_api_model` | `transcribe_api_model` | `prepare transcribe` | |
| `transcribe_api_key` | `transcribe_api_key_file` | `prepare transcribe` | A file, as for the janitor. |
| `transcribe_language` | `transcribe_language` | `prepare transcribe` | |

## What only Linux has

`data`, `listen`, `allow_remote`, `threads`, `advertise`, `allow_origin`, `console_admin`, `oauth_redirect`, `identity_listen`, `backup_dir` and `backup_keep` have no add-on option: the add-on fixes them (its data folder, its ports, Home Assistant's own backups). Each is a flag of the same name (`--data`, `--listen`, `--allow-remote`, `--threads`, `--advertise`, `--allow-origin`, `--console-admin`, `--oauth-redirect`, `--identity-listen`, `backup --to`, `backup --keep`).

## What only the add-on has

The ingress listener of Home Assistant (`--ingress-listen`, `--trust-ingress`) and the Home Assistant account linking are for Home Assistant: on Linux the page is opened with a console key or a tailnet identity ([LINUX.md](LINUX.md)).
