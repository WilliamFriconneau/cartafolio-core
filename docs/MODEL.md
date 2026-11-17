# MemCP model (the 0.9 refactor)

Status: design, decided in conversation, not implemented yet. Tests of 0.8.0 pass under the new name. The "Open an MCP" action (page and `open` command) is the one piece already built.

## Decisions that shape everything

- **A person's memory lives in one place.** No sync between devices, no swarm: every client reaches that one place through a proxy, over the local network or a tailnet.
- **No sharing of a vault between people.** No groups, no shares, no key distribution. A house agent is simply a person of its own.
- **No new MCP tool.** The tool list stays what it is: tags are a parameter of the existing write and search tools, the tag vocabulary comes back in `stats`, the guide travels in the handshake `instructions`. Unlocking, enrolling and approving are done on the page or the command line, never through a tool.
- **The part that is free stays small**: Rust, one binary per role, no model inside.

## People, vaults, connections

- A **person** owns a memory made of **vaults**. A vault is one SQLite file. A vault is the **unit of rights**: a connection with no right on a vault never opens its file.
- Vaults are created **only from a catalogue**, never freely, never by an assistant. The catalogue is data (a file), so another installation can ship its own.
- An **assistant** (chat client, coding agent, app) is a **connection** of a person, limited to some vaults.
- Words used everywhere: person, vault, connection, scope, guide, index, fact, tag. "Space", "agent", "device" and "token" are retired from the page.

## The catalogue is a tree with two kinds of nodes

- **Vault nodes**: the access boundary (for a life memory: me, health, people, home, money, work, hobbies; "pro" may be a vault of its own).
- **Tag nodes**: organisation inside a vault, `work/dev` for instance. An assistant may create any path under a vault, never a vault outside the catalogue.
- **Built**: the catalogue (a default of seven kinds, replaceable by a `catalogue.txt`), a `kind` on every vault (`custom` for those that predate it), `vault add KIND` and the page create a person's vault named after them and the kind, one of each, never by a free name (only an administrator, as operator, may name one, and it has no kind). The health and money tools are no longer a setting of the server: they are offered to a connection that reaches a vault of a kind carrying them, and the vault used is the one its person owns. `--healthapp-space` and `--finances-space` now only give a kind to an existing space.
- A right is granted on a vault only. A right limited to a tag path would need filtering on every read path (search, neighbours, duplicates, files, history), and each could leak. If added later it is a *filter*, labelled as such, with one test per tool.

## Facts: tags and concern

- Facts keep what 0.8 has (never edited, `supersede`, history, dedup, FTS5 search). They gain **hierarchical tags** (`a/b/c`, lowercase, up to 4 levels and 8 per fact); a tag matches itself and its descendants. Built: a `tags` parameter on `remember`, `supersede` and `recall`, the vocabulary with counts in `stats` (a fact counts for every level of its path), a correction that keeps the tags of what it corrects, and an error that states the grammar when a tag is wrong. A fact's tags are fixed when it is written: changing them means a correction.
- A fact belongs to **one vault**, which decides who may read it. It may **concern** other vaults with a reserved tag `@vault`: asking one vault then also finds, in the other vaults the caller can read, the facts that say they concern it. Concern never grants access: a vault that cannot be read is never opened (tested).

## The part written for the assistants: guide and index

- **Guide** (what `AGENTS.md` is): the person's standing instructions. It is the **only** content that is an instruction; everything else that comes back is data, flagged as such. It is written **only by the person**, never by an assistant (an assistant can only propose, the person confirms). One Markdown document per vault plus one at the person level, 16 KB at most, versioned.
- **Index** (what `MEMORY.md` is): a short map each vault keeps of itself. **Written by assistants**, replaced rather than edited, size-capped, and **data, not an instruction**.
- A connection receives the guide and index of the vaults it is opened on and of nothing else. Delivered in the handshake `instructions`; whether every client shows them is to be tested.
- **Built.** Guides are stored in the control data, versioned (a new text never overwrites; clearing keeps every text), at most 16 000 characters, for a person (`person`) and for each vault. They are written only through the admin page, by the person, by the owner of the vault, or by an administrator as operator, and no MCP tool can write one (there is no such tool, and a key that opens MCP is refused on the admin route). The index is an ordinary fact tagged `index`: the newest active one of a vault is told, `supersede` replaces it, and the section marker is broken inside it so that it cannot pass for an instruction section. The handshake carries the general rules, then the guide of the person and of each reachable vault (labelled as written by them: follow), then the index of each reachable vault (labelled as data, never instructions). A connection hears nothing about a vault it cannot reach, not even its name. The whole is bounded at about 48 000 characters: guides are never cut, indexes give way first and the handshake says so. This is also where a mistake of an assistant cannot become a lasting rule: what it writes can only be data.

## Apps

- An app such as a health tracker is an ordinary MCP client whose connection is limited to a vault of kind **health**. The server knows no app: no app dialect, no device streams.
- With no new tool, an app pushes **summaries as facts** (one fact per day for daily vitals, replaced when updated) and keeps its raw series on the device.
- The app works with no server; connected, it writes to the server and can read from it. What an app produces belongs to the app; what an agent writes belongs to the server. This is a client writing to a memory, not a sync.

## Shape of an installation

- The binary is a **single-person engine** by default: one data directory is one person's memory, with its owner, its connections and its rights. A standalone installation is exactly that.
- The Home Assistant app is a **manager**: it creates a person by starting one more engine on its own directory, shows the page (people, vaults, counters, open an MCP, export), signs people in (Home Assistant login) and routes each person's requests to their engine. It holds no memory itself.
- Why: isolation between people no longer depends on access-control code, and the multi-person code leaves the engine.
- What it costs: a **gateway** (routing, sign-in, token and OAuth endpoints) and a **supervisor** for the engines. The ports of an app are fixed in its configuration, so one front address has to route by path (or another scheme) to engines over local sockets. Unverified: OAuth discovery for clients that connect on a path prefix.
- Cut: engine = memory, tools, files; gateway = identity, tokens, OAuth, page, routing. The gateway is the only part that knows several people, and it never opens a vault.
- **Built (0.11.0).** `memcp-manager` (crate `manager`): a registry (an installation with no vault whose people are the people: names, Home Assistant login, administrator, the requests of unknown logins) and, for each person, a directory under `people/<name>` served by an ordinary engine that knows one person (a plain member: the administrators are the registry's). The manager runs them **in one process** for now (the engine is a library, an `App` per person); child processes behind the same routing are a later change, nothing else moves. It routes a request by a secret the request already carries (a key, a device token, an OAuth code or refresh token, a console token, asked of each person read-only through `holds_*`, which record nothing) or by the identity the listener vouches for (the ingress headers, the tailnet login). The OAuth clients register at the registry and are copied to the person who consents. The page shows the installation's people and requests to its administrators (merged into the state of their own page) and sends the people actions to the registry. The converter that brought the former one-directory installation over (a copy read, every vault checked fact by fact, the same keys kept) did its one job in production and was removed in 0.14.0; version 0.13.1 still carries it.
- **Still to do on this cut.** The engine still carries the code for several people (groups, shares, the request queue) that a person's directory never uses; it goes at the clean break. Engines as child processes. The registry's own page for an operator is the engine's page with its vault sections empty.
- **Before 0.11.0, the single binary did both jobs:** HTTP, OAuth and the admin page are in the server crate next to the memory. The target is the cut above: the engine speaks MCP over standard input and output only (the same engine serves a standalone installation, behind a small front, and a local assistant), and the gateway turns it into an address, sign-in, keys and a page. Splitting them is the 0.9 work.
- **One repository, one container, all in Rust.** A single Cargo workspace holds the engine crates, a manager crate (gateway, supervisor, page) and the packaging of the Home Assistant app. The app is one container: the manager and, as child processes, one engine per person. Changes that cross the engine and the manager are one commit, with one version number. Releases stay separate artifacts: the engine binary for a standalone installation, and one multi-architecture image for the app. If the two ever need different rhythms, the split is `git subtree split`, which keeps the history. The app has no Python since the mail, calendar and contacts tools left.
- **Decision (2026-11-01): memory only.** The mail, calendar and contacts tools (Python, started by the app beside the engine) and the upstream proxy that served them are removed from this repository. Mail, calendar and the house belong in a connector of their own, outside the app, added to an assistant next to the memory. The engine fetches, proxies and sends nothing.
- **How the app is built and distributed** (Home Assistant documentation): an app can name a prebuilt image (`image`, without a tag; the tag is always `version`), and without it the Supervisor compiles on the device, which is not workable for Rust. So a CI job builds the static binaries and publishes the image, and the app's `version` is the image tag. Home Assistant has no dependency between apps (`services` only covers `mqtt` and `mysql`): "the app needs the engine" can only mean the engine is inside the image.
- **Isolation inside the container**: each person's engine runs under its own Unix user with a data directory only that user can enter, so separate people are separated by the operating system, not by directories alone.
- **Exposure is the manager's job, in two modes.** Local: the gateway listens on the network, plain HTTP (clients that accept it; no cloud connector). Tailscale: either through the Tailscale app's `services` option (tailnet only, manual steps in the admin console, no Funnel), or through a node of its own, with the official `tailscale` command bundled in the image and run by the manager (own name, Funnel possible). The second is self-contained. Checked in Tailscale's documentation: Funnel needs MagicDNS, HTTPS certificates and a `funnel` node attribute in the tailnet policy, uses a name of the tailnet's own domain, listens on port 443, 8443 or 10000 only, works over TLS only, and needs a platform that can run the Tailscale command; and userspace networking (`tailscaled --tun=userspace-networking`, an auth key, ephemeral nodes recommended) is a supported way to run it where a container cannot make a tunnel device. **Not found in the pages read**: that `serve` and `funnel` themselves work in that mode. To try on a real app before relying on it. The manager never gets Supervisor management rights to edit another app's options; it shows the exact snippet to paste and tests whether its own public address answers.
- **Local assistants, standard input and output. Built.** `memcp mcp` serves MCP on standard input and output with the rights of a key made by `open` or `connect` (read from `MEMCP_TOKEN`): no port, no address. It is the same code as the HTTP side (one `dispatch`), so the rights, the guides, the tools and the limits are the same; the key is checked on every request and a revoked one stops it; nothing but protocol is written to standard output; a notification and a blank line get no reply. `open` prints the command to give the assistant. This is the engine's standalone way in, and the escape hatch for a later split into an engine and a gateway. The bridge the other way, for an assistant that only runs commands and reaches a memory elsewhere over the network, is `memcp-relay` (below).
- **The one erasure: a transcribed recording. Built.** A voice memo arrives as audio in the inbox (`file_put`, tag `inbox/audio`) when the device that took it could not transcribe it; a program (the server's `dictee.py`) transcribes it and deposits the text. Once that text is written, `file_erase {id, transcript}` erases the audio for good. It is the only thing this memory ever erases, and it is narrow: a recording (never a photo or a document), filed under `inbox/audio`, with a transcript that is an active text fact, in a vault the key can read (the text is sorted after it is written, so it may have left the recording's vault). The bytes go unless another record of the vault still holds them; the record (name, size, fingerprint, author), its retired fact and the reason (`erased: transcribed in <id>…`) stay as the trace, the export carries them and no bytes, and the file can no longer be read. The tool is offered to a key with write rights on a whole vault, never to one limited to paths (the Bridge, HealthApp) nor to a read-only key.
- **Local clients, the bridge. Built, in its own repository** (memcp-capture). `memcp-capture` is the one program on the side of the person who captures, on Linux and on macOS: the memory's tools on standard input and output (or over HTTP on this machine) for an assistant that runs a command, with an outbox in front that keeps the writes that need no decision while the network is down (`remember` with the tag `inbox`, `deposit`, `file_put`; the answer says `queued`, never `created`), four tools of its own (`capture_text`, `capture_file` by path inside folders the person named, `outbox_status`, `outbox_flush`), and commands to put captures into an inbox. A Mac app is a thin SwiftUI shell on the same Rust core. It is built on `memcp-client` (`crates/client` here), the project's one TLS client (`ureq` with rustls, allowed in `deny.toml` for it alone): the server still has none.
- **Installed apps and data**: an app installed from a repository is a different app from one installed locally (different slug), so its data folder is new. Moving from the current local app is a one-time copy of the data folder while both are stopped, after a Home Assistant backup.

### Build or reuse

- **Engine** (facts never edited, history, tags, guide and index, health record, bank import, files): built, already exists. No existing memory server was found that has these; the generic parts an adopted engine would bring (storage, search, tools) are the smallest part of the code.
- **Reuse, do not hand-roll, where the code is security-critical**: the OAuth 2.1 server with dynamic client registration, passkeys, TLS and public exposure. The current OAuth code has had no independent audit. Evaluate mature components first (a standalone OAuth/OIDC server, a vetted WebAuthn library, the tailnet's own HTTPS and identity), and keep our own only if integration cost or fit rules them out. A WebAuthn library that needs OpenSSL would break the static binaries.
- **Reuse** for storage and backup: SQLite and FTS5, Home Assistant for backup.

## Storage layout, backup and portability

- One **directory per person**, one SQLite file per vault inside it, and the person's file blobs next to them. Isolation between people and between vaults stays physical.
- A person's directory is **self-describing and self-contained**: each vault file records its own name, kind, owner and format version, and no path is absolute. Moving a person to another installation is a copy of the directory followed by `adopt`. This is a move, not a sync.
- **Never in the portable part**: tokens, connections, sign-in links, rights. They belong to an installation.
- **Backup is the reason to run as a Home Assistant app**: everything (control data, vault files, blobs) lives under the app's data folder, so one Home Assistant backup, taken cold and consistent, is a full restore. Keep that invariant. A standalone installation gets the same from the `backup` command on a schedule.
- **Export. Built.** `export` (command line, and "export my data" on the page, which gives each person only their own) writes, for each vault the person **owns**: a consistent copy of its database, its files, a readable Markdown version (the facts in force, then what was replaced or retired with the reason, the tags, the health record entries), plus the person's instructions in every version and a manifest with the fingerprint of every file. It holds no key, connection or right, and nothing of a vault shared with the person by someone else. `verify-export` checks one without any installation. `adopt` takes the vaults into an installation as the property of a person: it checks every fingerprint, refuses before changing anything if a name is taken (a `--prefix` renames), checks the counts of each vault after copying it, and undoes everything if a step fails. Tested by a round trip: facts, tags, history, files and instructions identical on the other side. Delete is one operation on one directory. An export that works is what makes the choice reversible, so it exists before anyone is asked to commit.
- One copy lives in one place, so that place is the single point of failure: keep a Home Assistant backup copied off the machine and an occasional export.

## Encryption (optional, last)

An assistant that reads a memory must see it in clear at that moment, and a cloud model receives the text it is given. So "encrypted" can only mean at rest, on a server the person trusts: each vault file encrypted with a key the server holds only while it is unlocked. It protects a stolen disk and a backup that leaves the machine, not the host while running. Inside the Home Assistant app, Home Assistant's own backup encryption already covers the backups, so the gain there is small; the case that needs it is an installation where the administrator should not read other people's data.

If built: the whole **file** is encrypted, not columns (the search index holds the stems of every text); one master key per person, one key per vault, and for a scoped connection only the keys of the vaults it is opened on; a **recovery code** of 24 words shown once, with some words typed back at setup; losing every wrapper means the data is gone, and the page says so.

## One address per connection

- **Built (0.13.1).** Every connection has an address named after it, `<public url>/mcp/<name>`, and there is no address for "the whole memory": the bare `/mcp` answers 404. The name is chosen first, in the steps that connect an assistant (or, for a web connector, it is the end of the address given to it, which becomes the connection's name at the consent). A key only works at its own connection's address; what it reaches is what its rights allow, and cutting vaults from them later changes neither the address nor the sign-in. `/mcp/<vault>` still exists: a key used there is seen through that vault alone. A connection and a vault never share a name, and the name of a revoked connection can be taken again.
- The 401 points at the metadata of that very address (`/.well-known/oauth-protected-resource/mcp/<name>`, which names that resource); the consent page receives the address as the `resource` of the request (RFC 8707) and creates the connection of that name, with nothing ticked beforehand and no upstream tools. What the specification says is checked; that claude.ai follows it for an address with a path is checked with a real connector.

## Breaking change policy

**One clean break, later.** Some tables no longer have any code behind them (`devices`, `device_*`, `outbox`, `record`, `embeddings`, `groups`, `group_*`, `pairing_codes`) and the `upstream` columns. They are kept because an installation may hold data in them (the health vault of this one holds what the HealthApp app pushed before 0.12: it can be pushed again from the phone, and nothing is erased for anyone). Dropping them means exporting that data first and restarting the schema at version 1, which an older program refuses with a clear message. Until then the server neither reads nor writes them.

Until then every change stays additive and carries a test that rewinds a database to the earlier version and checks that it upgrades without loss.

## Considered and set aside

- **Sync between devices, swarm, zero-knowledge relay.** Dropped: one place per person, a merge written without an audit is a risk to a person's whole memory, and the Home Assistant backup covers durability. If offline use with a local model becomes real, the conditions are written down: every change numbered by (origin node, sequence), a fork representable (a fact may have several successors), uniqueness of a text scoped to a vault, tags as a set, a restored node taking a new node id, the guide accepted only from its owner's devices, an explicit erasure event, and convergence tests (any order, any duplicate, same state). Carry change files with an existing file-sync tool, never the SQLite files.
- **A hash-chained log as the source of truth.** Its value was backup and sync; without sync the database, the Home Assistant backup and the readable export are enough.
- **Markdown files as the source of truth, mem0, palace-rs, Basic Memory, engram as the engine.** Each lacks something we need or adds a cost we reject (a model on every write, a Docker stack, a copyleft licence, no HTTP server, editing and deleting by the assistant).
- **Sharing, groups, per-fact rights, passwords.** (Rights on a tag path were set aside too, then built on 2026-11-01 because several apps deposit into one vault: see "Rights by path".)


## Sources, dossier and inbox (decided 2026-11-01, built in 0.10.0)

The vault stores. It imposes no health format and runs no health expertise. A person's data comes
from several apps; an expert reader (a health app, or any other) makes sense of it.

- **Layers.** `sources/<app>`: raw deposits of an app, duplicates between apps kept ("better a
  duplicate than a gap"). `dossier/<app>`: the sorted memory an expert reader writes, one entry per
  measure with the spread between sources. `inbox`: quick notes to sort later. The raw layers
  (`sources`, `inbox`) are left out of a plain `recall`; the dossier is what it finds. Each
  connection is told at the handshake how many raw records and notes there are and how to read them,
  so nothing is invisible even if no one has written a dossier.
- **`origin`, a parameter of `remember`, not a new tool**: `{ app, id, at, code, layer }`. The same
  app and id again is a no-op or replaces the app's own earlier version (history kept); the answer
  carries a cursor (latest `at` held for that app and layer, and a count), and `stats` lists them
  under `sources` and `dossier`: the app resumes from there. A feed of alike records is never asked
  the "close facts" question. Identical text held under another id or source is said
  (`same_text_held`), never silently lost. The deduplication is per source and per id; deciding that two
  readings are one measurement takes health knowledge (a time window per measure, units) and is the
  reader's, never the vault's.
- **Standard codes.** `code` is `system:code` (`loinc:8867-4`), filed as `code/loinc/8867-4`. The
  vault carries no table of codes and checks only the shape, so there is no code-list licence to
  carry. FHIR's vocabularies (LOINC for what, UCUM for the unit) are used as vocabulary; FHIR itself
  is not the storage format, and is a candidate for export and import. Not built: a structured
  `value` and `unit` on a record, which an export to FHIR `Observation` would need.
- **Inbox.** Tag `inbox` (or `inbox/...`): no "close facts" question; sorting a note is a `supersede`
  with the clean fact and its real tags, the raw note stays in the history.
- **A health app** is one source and, if it wants, the expert reader of the health vault: it keeps its referentials and its expertise, deposits its dossier with `layer: dossier`, and reads assistants' suggestions as facts tagged `proposals/<app>`. Its expert tools are a connector of its own, a client of the vault. The vault knows no app: it has no route, no tool and no format of any. See [HEALTH-SOURCES.md](HEALTH-SOURCES.md).
- **Kind tools.** Only the bank statement tools (`finances_*`, for a vault of the money kind) remain. The health tools, the HealthApp dialect (`/healthapp/v1/*`, the `healthapp_*` tools) and the devices (pairing, inbox, outbox) are gone: the memory knows no app and imposes no health format.
- **Rights by path (built).** A grant can be limited to a tag path inside a vault:
  `health/sources/bevel:rw`. Such a connection reads and writes only the facts filed under that
  path (a tag matches itself and its descendants), and gets the five tools that can be bounded
  (`recall`, `remember`, `retire`, `file_put`, `stats`; `stats` shows only its own cursor and no
  counts or vocabulary). A write goes under its single path whatever tags it asks for, and must
  say which when it has several; another app's source, or the dossier, is refused. Facts outside the
  paths are never shown to it: an identical text held elsewhere is "already held" with no fact, close
  facts elsewhere are neither listed nor asked about, retiring one is "unknown fact". A vault is
  granted whole or by paths, never both. Stored in `grant_paths` (control version 9); the vault's own
  `grants` row says it reaches the vault at all.
