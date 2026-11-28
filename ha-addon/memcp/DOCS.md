# Pensine

A durable memory for your AI assistants (Claude, Gemini, a local model), served over MCP. **One memory per person**: each person has a directory of their own, spaces of their own, and keys that never go beyond them. The add-on creates and administers the people; it stores no memory of its own.

## First start

1. Set the options (below) and start the add-on, then open **Memory** in the sidebar.
2. The first Home Assistant user who opens it becomes the administrator and gets a memory of their own. **Every other Home Assistant user who opens it gets a memory of their own at once**, empty and private: nobody else reads it, and the administrator sees only that they exist. With `new_people: ask`, a user you did not add waits instead, and you let them in (or not) under "Waiting to be let in".
3. A person with no memory yet sees exactly two choices: **Start a memory** (the spaces personal, health, home, finances, hobbies and work are made, the person is named, and one address that carries the key connects an assistant) or **Join a memory** (the address of another node, or one found on the network, then three words compared on both machines). Each path has at most four steps, with the progress shown. Afterwards the page opens on the machines that hold the memory, live, then the spaces, the keywords and targets of each, and "Connect an assistant". Joining another memory needs the `memcp-sync` program, shipped in this add-on. The administrator also sees the people.

| option | meaning |
|---|---|
| `language` | The search language of a new memory (`en` or `fr`). |
| `strict_write_agents` | Connections that must read the memory within 30 minutes before writing. Use a list of names or `["*"]` to cover all except apps, peers, consoles and the janitor. Empty by default. |
| `admins` | Home Assistant logins that administer the installation. Empty: the first login. |
| `new_people` | `auto` (the default): every Home Assistant user gets a memory of their own the first time they open it. `ask`: a user you did not add waits for you. |
| `public_url` | The address assistants reach, `https://…`. |
| `identity_trust`, `login_url`, `tailnet_users` | Sign-in of Claude on the web through Tailscale, below. |

## People, and not losing a memory

The administrator's page opens on **People**: each person, their role, whether they are linked to a Home Assistant account, how many spaces and live connections they have, and what you can do (make administrator or member, disable, unlink, console key). To add someone, give the **name of their Home Assistant login**: they are linked to it the first time they open Memory. Someone who opens Memory without being added is let in at once with a memory of their own (or waits for you, with `new_people: ask`).

Deleting a Home Assistant account never deletes the person or their memory: it stays here, "linked" to an account that no longer exists. You can then **unlink** the person (their memory stays), and the next login of that name takes it up again. Disabling a person stops their keys at once and erases nothing.

Each person can protect their own memory, on their page, under **Safety**:
- **An heir**: another person of the installation. After 30 days without the person opening their page, the heir can attach a new, waiting Home Assistant account to their memory. Until then the heir sees nothing of it.
- **A recovery key**, shown once: a new Home Assistant account that enters it on the waiting page takes the person's memory over, once. A new key cancels the old one.

**If the administrator loses their own account:** put the new account's login name in the `admins` option and restart: the person of that name is linked to the new account. Or, from a terminal on the Home Assistant host, `docker exec -it addon_local_memcp pensine-manager --data /data/manager console-token <name>` prints a key that opens the registry's page on port 8791 of the local network (never through the internet).

The memory is not encrypted at rest: whoever can read the disk can read it. Protect the host and the backups.

## Connecting an assistant

The page asks the connection's name first: it is the end of its address, `http://<home-assistant>:8791/mcp/<name>`. It then shows the token once and the line to paste (or, for HealthApp, a configuration to paste). Any MCP client with a bearer header works: that address, header `Authorization: Bearer <token>`. A key only works at its own connection's address (or at `/mcp/<space>`, which is limited to that space); the bare `/mcp` answers nothing. The key says whose memory it is.

Port 8791 carries tokens in clear: reach it through Tailscale or a TLS proxy, not from the internet.

## Backups

The add-on uses cold backups: Home Assistant stops it for a few seconds while it copies the databases, so every backup is consistent.

## One place for the memory

The add-on holds the memory of each person, one space per subject: the health record that apps deposit into, the bank statements, and the rest. It stores and finds what it is given; it fetches and proxies nothing. Mail, calendar and the house are a connector of their own, outside the add-on, added to an assistant next to this one. Nothing else has to run for the add-on, so Home Assistant can start alone with the whole memory.

A health app deposits into a person's health space, under its own path: see [HEALTH-SOURCES.md](https://github.com/WilliamFriconneau/pensine-core/blob/main/docs/HEALTH-SOURCES.md) for what an app should send.

### Tailscale

A space has a kind (`memcp space kind NAME health`, or made from the catalogue with `vault add health`): the health record tools are offered to connections that reach a space of that kind. `identity_trust`, `login_url` and `tailnet_users` serve the sign-in page of Claude on the web and the node page for the tailnet, on port 8792. The node believes the `Tailscale-User-Login` header only from the one address in `identity_trust`, the machine running Tailscale serve; any other caller gets 403. `tailnet_users` maps a tailnet login to a person (`login=person`). Never publish port 8792 through Funnel.

## Bank statements

A space of the money kind (`vault add money`, or `space kind finances money` for one that exists) carries the bank statement tools: `finances_import`, `finances_balances`, `finances_movements`, `finances_monthly` and `finances_rules`. Upload the bank's `.xlsx` export (a summary sheet and one sheet per account) from the Files section of the node page, then ask an agent to import it: only the new movements are added, so an overlapping export is safe. The person's own rules (a monthly cap on a merchant, a merchant to watch, the words of an internal transfer) are set with `finances_rules` and stay in the space.

## Moving from another app

To bring in the memory of an installation that ran as another app (an older version under another name, say): stop that app, take its data folder (`manager/`, and `janitor/` if it had one) from a Home Assistant backup of it, and put them in this app's configuration folder (`/addon_configs/<this app>/import/manager`, `…/import/janitor`) before the first start. On its first start, with no data of its own, this app takes them in, keeps the keys and the addresses of every connection, and renames the folder `import.done-…`. Check the page, then delete the old app. The databases are brought up to date by themselves the first time they are opened, so keep the backup: an older version cannot open them afterwards.

## Voice notes and attachments

On the page of a space, **Record** keeps a voice note, **Attach files** keeps photos, PDF, text and recordings; each is a note of the space, with its file. The microphone needs the page to be served over https (Home Assistant's own address is, in the app or through Nabu Casa or your own certificate).

A voice note waits as a recording under `inbox/audio` until something reads it. The node reads it itself (there is no other program): turn on `transcribe` and name a speech service of the OpenAI kind (`transcribe_api_url`, for example `https://api.openai.com/v1/audio/transcriptions`, or a service on your network such as a faster-whisper server; `transcribe_api_model`, `transcribe_api_key`). **The recording leaves this machine for that service**: turn it on only for a service you trust. The text becomes a note of the same space, and the recording is erased a short while after its text is filed.

| Option | Meaning |
|---|---|
| `transcribe` | on or off |
| `transcribe_person` | whose recordings (defaults to `intake_person`, then `janitor_person`) |
| `transcribe_spaces` | the spaces whose recordings are read, separated by commas (empty: every space of the person, those made later included, `all` says the same) |
| `transcribe_api_url`, `transcribe_api_model`, `transcribe_api_key` | the speech service |
| `transcribe_language` | `fr`, `en`… or empty to let the service detect it |
| `transcribe_hints` | on by default: the node tells the speech service the words your memory says you use (names that recur in your notes, the keywords and vocabularies of your spaces, health terms), as the `prompt` of the request. **Those words leave this machine with the recording**; turn it off to send the recording alone |

## Watched folders

`watch` is a list of folders, each written `FOLDER -> SPACE` (for example `/share/dictation -> personal`). A file added to a watched folder becomes a note (Markdown and text files), a recording (audio files, read by the node as above) or a file of that space, tagged `inbox`, where the janitor files it like any other note. Without `-> SPACE` it goes to the person's Inbox. The files stay where they are: nothing in a watched folder is moved, changed or deleted. A folder is taken in whole the first time it is looked at, so point it at a folder made for the purpose. The folders must be under `/share` or `/media` (the app maps both). `intake_person` names the person (the name in Memory) these folders and the voice notes work for; empty, it is `transcribe_person`, then `janitor_person`.

## The janitor (optional)

Off by default. When it is on, it files the notes that wait in the Inbox of one person and under the `inbox` tag of their spaces, never for nothing: once the writing
of a note tagged `inbox` has calmed down (`janitor_quiet` seconds after the last write, 120 by default), when an assistant asks for it
(the `tidy` tool, « range ma mémoire »), and once a night (`janitor_night`, 03:00 by default):
what the devices sent to **Inbox** (the person's one box for everything that comes in) goes to
the right space, tagged `inbox`, or waits on the person's page when the janitor is unsure; each note tagged `inbox` in a space is filed,
its text unchanged, in that space; and a piece of a note absorbed from a folder becomes a few short, dated facts. It reads recordings no more:
the node transcribes them itself (see above).

| option | what |
|---|---|
| `janitor` | on or off |
| `janitor_person` | whose Inbox and spaces (the person's name in Memory) |
| `janitor_inboxes` | the spaces whose `inbox` tag it files (by default all of them; Inbox is always read) |
| `janitor_work_words` | your own words for work (a product, a company, a team): a capture that holds one goes to your `work` space from Inbox, before any model. A recording whose name says `reunion` or `meeting` goes there too. Empty by default |
| `janitor_api_url`, `janitor_api_model`, `janitor_api_key` | a service that speaks the OpenAI chat format (Mistral in the EU: `https://api.eu.mistral.ai/v1/chat/completions`, `mistral-small-2603`) |
| `janitor_ollama_url`, `janitor_ollama_model` | a local model on the network (Ollama), tried after the API, or alone |

It makes its own connection, `janitor`, reaching all the person's spaces, the first time it starts; its key stays in the
app's data. With an API, what it reads leaves the house for that service: choose one whose terms you accept.

## Found without typing an address

The node can advertise itself on the local network (service `_memcp._tcp`) only on the host network, and the host network of this app clashes with the ingress port of Home Assistant on some installations, so it is not set. The apps find the node by its address (local or tailnet) or by the page. `node_name` is the name shown to the apps.
