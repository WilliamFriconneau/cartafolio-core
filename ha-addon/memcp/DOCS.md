# MemCP

A durable memory for your AI assistants (Claude, Gemini, a local model), served over MCP. **One memory per person**: each person has a directory of their own, vaults of their own, and keys that never go beyond them. The add-on creates and administers the people; it stores no memory of its own.

## First start

1. Set the options (below) and start the add-on, then open **Memory** in the sidebar.
2. The first Home Assistant user who opens it becomes the administrator and gets a memory of their own. **Every other Home Assistant user who opens it gets a memory of their own at once**, empty and private: nobody else reads it, and the administrator sees only that they exist. With `new_people: ask`, a user you did not add waits instead, and you let them in (or not) under "Waiting to be let in".
3. A person with no memory yet sees exactly two choices: **Start a memory** (the spaces personal, health, home, finances, hobbies and work are made, the person is named, and one address that carries the key connects an assistant) or **Join a memory** (the address of another node, or one found on the network, then three words compared on both machines). Each path has at most four steps, with the progress shown. Afterwards the page opens on the machines that hold the memory, live, then the spaces, the keywords and targets of each, and "Connect an assistant". Joining another memory needs the `memcp-sync` program, shipped in this add-on. The administrator also sees the people.

| option | meaning |
|---|---|
| `language` | The search language of a new memory (`en` or `fr`). |
| `admins` | Home Assistant logins that administer the installation. Empty: the first login. |
| `new_people` | `auto` (the default): every Home Assistant user gets a memory of their own the first time they open it. `ask`: a user you did not add waits for you. |
| `public_url` | The address assistants reach, `https://…`. |
| `identity_trust`, `login_url`, `tailnet_users` | Sign-in of Claude on the web through Tailscale, below. |

## People, and not losing a memory

The administrator's page opens on **People**: each person, their role, whether they are linked to a Home Assistant account, how many vaults and live connections they have, and what you can do (make administrator or member, disable, unlink, console key). To add someone, give the **name of their Home Assistant login**: they are linked to it the first time they open Memory. Someone who opens Memory without being added is let in at once with a memory of their own (or waits for you, with `new_people: ask`).

Deleting a Home Assistant account never deletes the person or their memory: it stays here, "linked" to an account that no longer exists. You can then **unlink** the person (their memory stays), and the next login of that name takes it up again. Disabling a person stops their keys at once and erases nothing.

Each person can protect their own memory, on their page, under **Safety**:
- **An heir**: another person of the installation. After 30 days without the person opening their page, the heir can attach a new, waiting Home Assistant account to their memory. Until then the heir sees nothing of it.
- **A recovery key**, shown once: a new Home Assistant account that enters it on the waiting page takes the person's memory over, once. A new key cancels the old one.

**If the administrator loses their own account:** put the new account's login name in the `admins` option and restart: the person of that name is linked to the new account. Or, from a terminal on the Home Assistant host, `docker exec -it addon_local_memcp memcp-manager --data /data/manager console-token <name>` prints a key that opens the registry's page on port 8791 of the local network (never through the internet).

The memory is not encrypted at rest: whoever can read the disk can read it. Protect the host and the backups.

## Connecting an assistant

The page asks the connection's name first: it is the end of its address, `http://<home-assistant>:8791/mcp/<name>`. It then shows the token once and the line to paste (or, for Kalios, a configuration to paste). Any MCP client with a bearer header works: that address, header `Authorization: Bearer <token>`. A key only works at its own connection's address (or at `/mcp/<vault>`, which is limited to that vault); the bare `/mcp` answers nothing. The key says whose memory it is.

Port 8791 carries tokens in clear: reach it through Tailscale or a TLS proxy, not from the internet.

## Backups

The add-on uses cold backups: Home Assistant stops it for a few seconds while it copies the databases, so every backup is consistent.

## One place for the memory

The add-on holds the memory of each person, one vault per subject: the health record that apps deposit into, the bank statements, and the rest. It stores and finds what it is given; it fetches and proxies nothing. Mail, calendar and the house are a connector of their own, outside the add-on, added to an assistant next to this one. Nothing else has to run for the add-on, so Home Assistant can start alone with the whole memory.

A health app deposits into a person's health vault, under its own path: see [HEALTH-SOURCES.md](https://github.com/WilliamFriconneau/memcp-core/blob/main/docs/HEALTH-SOURCES.md) for what an app should send.

### Tailscale

A vault has a kind (`memcp space kind NAME health`, or made from the catalogue with `vault add health`): the health record tools are offered to connections that reach a vault of that kind. `identity_trust`, `login_url` and `tailnet_users` serve the sign-in page of Claude on the web and the admin page for the tailnet, on port 8792. The server believes the `Tailscale-User-Login` header only from the one address in `identity_trust`, the machine running Tailscale serve; any other caller gets 403. `tailnet_users` maps a tailnet login to a person (`login=person`). Never publish port 8792 through Funnel.

## Bank statements

A vault of the money kind (`vault add money`, or `space kind finances money` for one that exists) carries the bank statement tools: `finances_import`, `finances_balances`, `finances_movements`, `finances_monthly` and `finances_rules`. Upload the bank's `.xlsx` export (a summary sheet and one sheet per account) from the Files section of the admin page, then ask an agent to import it: only the new movements are added, so an overlapping export is safe. The person's own rules (a monthly cap on a merchant, a merchant to watch, the words of an internal transfer) are set with `finances_rules` and stay in the space.

## Moving from another app

To bring in the memory of an installation that ran as another app (a former HemCP, say): stop that app, take its data folder (`manager/`, and `janitor/` if it had one) from a Home Assistant backup of it, and put them in this app's configuration folder (`/addon_configs/<this app>/import/manager`, `…/import/janitor`) before the first start. On its first start, with no data of its own, this app takes them in, keeps the keys and the addresses of every connection, and renames the folder `import.done-…`. Check the page, then delete the old app. The databases are brought up to date by themselves the first time they are opened, so keep the backup: an older version cannot open them afterwards.

## Voice notes and attachments

On the page of a space, **Record** keeps a voice note, **Attach files** keeps photos, PDF, text and recordings; each is a note of the space, with its file. The microphone needs the page to be served over https (Home Assistant's own address is, in the app or through Nabu Casa or your own certificate).

A voice note waits as a recording under `inbox/audio` until something reads it. To read them here, turn on `transcribe` and name a speech service of the OpenAI kind (`transcribe_api_url`, for example `https://api.openai.com/v1/audio/transcriptions`, or a service on your network such as a faster-whisper server; `transcribe_api_model`, `transcribe_api_key`). **The recording leaves this machine for that service**: choose one you trust. The text is put in the same space and the recording is erased a week later. Without a service here, a computer that runs the desktop bridge with a local speech model reads the same recordings, and nothing leaves your network.

| Option | Meaning |
|---|---|
| `transcribe` | on or off |
| `transcribe_person` | whose recordings (defaults to `janitor_person`) |
| `transcribe_spaces` | the spaces whose recordings are read, separated by commas (by default the person's À trier) |
| `transcribe_api_url`, `transcribe_api_model`, `transcribe_api_key` | the speech service |
| `transcribe_language` | `fr`, `en`… or empty to let the service detect it |

## The janitor (optional)

Off by default. When it is on, it files the notes that wait in the inboxes of one person, never for nothing: once the writing
into an inbox has calmed down (`janitor_quiet` seconds after the last write, 120 by default), when an assistant asks for it
(the `tidy` tool, « range le coffre »), and once a night (`janitor_night`, 03:00 by default):
what the devices sent to **À trier** (the person's one box for everything that comes in, which belongs to no vault) goes to
the inbox of the right vault, or waits on the person's page when the janitor is unsure; each note of a vault's inbox is filed,
its text unchanged, in that vault; and a piece of a note absorbed from a folder becomes a few short, dated facts. It reads recordings no more:
the bridge, where the speech model is, transcribes them.

| option | what |
|---|---|
| `janitor` | on or off |
| `janitor_person` | whose inboxes (the person's name in Memory) |
| `janitor_inboxes` | the vaults whose inbox it files (by default all of them; À trier is always read) |
| `janitor_work_words` | your own words for work (a product, a company, a team): a capture that holds one goes to your `work` vault from À trier, before any model. A recording whose name says `reunion` or `meeting` goes there too. Empty by default |
| `janitor_api_url`, `janitor_api_model`, `janitor_api_key` | a service that speaks the OpenAI chat format (Mistral in the EU: `https://api.eu.mistral.ai/v1/chat/completions`, `mistral-small-2603`) |
| `janitor_ollama_url`, `janitor_ollama_model` | a local model on the network (Ollama), tried after the API, or alone |

It makes its own connection, `janitor`, reaching all the person's vaults, the first time it starts; its key stays in the
app's data. With an API, what it reads leaves the house for that service: choose one whose terms you accept.

## Found without typing an address

The app uses the host network, so that the node advertises itself on the local network (service `_memcp._tcp`). An app on the same network finds it by itself and asks to join; the person answers on the page after comparing three words. `node_name` is the name shown to the apps. A phone or computer on another network (another VLAN, another site) does not see the advertisement: use the tailnet address or type the address.
