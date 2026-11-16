# MemCP

A durable memory for your AI assistants (Claude, Gemini, a local model), served over MCP. **One memory per person**: each person has a directory of their own, vaults of their own, and keys that never go beyond them. The add-on creates and administers the people; it stores no memory of its own.

## First start

1. Set the options (below) and start the add-on, then open **Memory** in the sidebar.
2. The first Home Assistant user who opens it becomes the administrator and gets a memory of their own. **Every other Home Assistant user who opens it gets a memory of their own at once**, empty and private: nobody else reads it, and the administrator sees only that they exist. With `new_people: ask`, a user you did not add waits instead, and you let them in (or not) under "Waiting to be let in".
3. On their page, a person makes their vaults (for instance *me*, *health*, *work*, *home*), connects assistants with "Connect an assistant", writes their instructions, and can export everything. The administrator also sees the people.

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

A health app deposits into a person's health vault, under its own path: see [HEALTH-SOURCES.md](https://github.com/WilliamFriconneau/memcp/blob/main/docs/HEALTH-SOURCES.md) for what an app should send.

### Tailscale

A vault has a kind (`memcp space kind NAME health`, or made from the catalogue with `vault add health`): the health record tools are offered to connections that reach a vault of that kind. `identity_trust`, `login_url` and `tailnet_users` serve the sign-in page of Claude on the web and the admin page for the tailnet, on port 8792. The server believes the `Tailscale-User-Login` header only from the one address in `identity_trust`, the machine running Tailscale serve; any other caller gets 403. `tailnet_users` maps a tailnet login to a person (`login=person`). Never publish port 8792 through Funnel.

## Bank statements

A vault of the money kind (`vault add money`, or `space kind finances money` for one that exists) carries the bank statement tools: `finances_import`, `finances_balances`, `finances_movements`, `finances_monthly` and `finances_rules`. Upload the bank's `.xlsx` export (a summary sheet and one sheet per account) from the Files section of the admin page, then ask an agent to import it: only the new movements are added, so an overlapping export is safe. The person's own rules (a monthly cap on a merchant, a merchant to watch, the words of an internal transfer) are set with `finances_rules` and stay in the space.

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
