# API changes, for the apps

Every new route, tool or JSON form, with the version that brings it. The apps read this file. Where a name changed, the node reads both and writes the new one, unless it says otherwise.

## The command line reaches a running node (next version)

Every command of `memcp` also runs against a node that is up, with a key ([CLI.md](CLI.md)). All routes are under `/admin/api`, authorised by a console key (`Authorization: Bearer hcpc_...`), with `X-Requested-With: memcp` on a POST, and audited.

- **`POST /admin/api/cli`** `{ "argv": ["space", "new", "Cars"] }` runs a command of the command line for the key's person, on the node's data, and answers `{ "code": 0, "stdout": "...", "stderr": "..." }`: exactly the text and exit code of the local command (`code` 1 for a failure of the command, 2 for a wrong command line). A command of the machine itself (`init`, `serve`...) answers 400 with the reason; a command that needs an administrator, 403 with a sentence; a command a member runs on another person (`--user`), 403. An administrator acts for anyone with `--user`; any other key acts for its own person. The files a command reads must have been sent first (`@upload:ID[/path]` words).
- **`POST /admin/api/upload?id=&path=&offset=`** takes one chunk of one file (raw bytes, up to 32 MB; the client sends 8 MiB): `id` is 8 to 32 letters or digits, `path` a plain relative path, `offset` the size of the file now (409 otherwise: a chunk is never written twice). 8 GiB at most per `id`, 200 000 files; a folder not used within 12 hours is removed.
- **`POST /admin/api/upload/discard`** `{ "id" }` forgets a folder that was sent.
- **`GET /admin/api/export`** now sends the tar from a file, block by block, instead of holding it in memory; the answer is the same.
- **`POST /admin/api/join/invite`** `{}` answers `{ "ticket", "expires_in", "single_use": true }`: the one-time ticket `POST /mcp/<peer>/join/invite` makes, for the signed-in person (a console key), so that a person can invite a device from the command line. Six a minute.
- **`POST /admin/api/join-memory`** takes an optional `ticket` (a ticket from a node of the same person): it is given to `memcp-sync` in `MEMCP_JOIN_TICKET`, never in its arguments.
- **`GET` and `POST /admin/api/space/folder`** are named by the command `memcp folder get|set` and are **pending** (they come with the space folders): the command answers exit 6, "pending", on a 404.
- Command line only: `memcp status`, `memcp person new`, `memcp note recall`, `memcp note tags`, `memcp connection list|new|regrant|revoke|internal|readonly|person-guide|limit` (the former words still work), `memcp invite ...`, `memcp join ...`, `memcp folder ...`. `--prefix` and `--token-env` are read as options with a value (they were read as switches, so `adopt DIR --prefix P` did not apply the prefix).

## « À trier » is called Inbox (next version)

Only words change; no route, field, kind or stored name does.

- The person's box is shown as **Inbox** (the same word in French and Spanish). It is still a space of kind `a-trier` named `<person>-a-trier`; `triage: true`, the `triage` answer of `POST /sync/spaces`, `S.triage` of the page's state and the `triage_*` tools keep their names. An app shows the label « Inbox » for the space whose kind is `a-trier`, never its stored name.
- Texts an app may have matched change: errors say « the Inbox is not a space to change or delete », « the Inbox is made for you already », « a space cannot become the Inbox », « `<space>` is not the person's Inbox »; the reason written in a note's history by `triage_send` is « sent from the Inbox to `<space>` (`<id>`) by `<author>` »; the janitor's note in the box says « attendent dans l'Inbox ». Match the kind and the tags, not these texts.
- `memcp_spaces::parse_spaces` reads a default space labelled « À trier » as « Inbox » (`memcp_spaces::FORMER_DEFAULT_LABEL`); its id and path do not change.
- `doctor` says « space NAME: … » where it said « vault NAME: … », and « this node answers on ADDR » where it said « the server answers on ADDR »; its check names (`vault-file`, `server-answers`…) are unchanged.
- Command line: `memcp space make KIND` (the former `vault add KIND`, still accepted), `memcp import --folder DIR` (the former `--vault DIR`, still accepted).

## The swarm: spaces, « À trier » and archives go both ways (20261117.0.17)

The protocol stays 1: everything below is added, and a node of the previous version is kept in step as before ([FEDERATION.md](FEDERATION.md), *Spaces in the swarm*).

**`GET /mcp/<connection>/sync/spaces`** carries more. Each space gains `label`, `parent`, `position`, `since` (seconds: when the space was made at its origin, or brought back from an archive) and `triage` (`true` for the person's own « À trier »). The answer gains `offers` (whether this connection may offer spaces) and `archived` (the archives of the person's spaces this node knows of):

```json
{ "protocol": 1, "version": "…", "offers": true,
  "spaces": [
    { "space": "owner-a-trier", "kind": "a-trier", "head": 12, "writable": true,
      "label": null, "parent": null, "position": 0, "since": 1794800000, "triage": true },
    { "space": "owner-pompiers", "kind": "custom", "head": 5, "writable": true,
      "label": "Pompiers", "parent": "hobbies", "position": 2, "since": 1794800500, "triage": false } ],
  "archived": [ { "space": "owner-garden", "archived_at": 1794801000 } ] }
```

**`POST /mcp/<connection>/sync/spaces`**, new. Only a machine of the person: a `peer-…` connection that reaches every space of theirs, whole, and may write (otherwise 403). Two bodies:

- A space the peer lacks: `{"space", "kind", "label", "parent", "position", "since"}`. Answers `{"result": "created", "space"}` (made now, for the same person), `{"result": "held", "space"}` (already there), `{"result": "triage", "space": "<this node's box>"}` (the offer was « À trier » and this node has the person's box: the two are paired), or `{"result": "archived", "archived_at"}` (this node archived it after the offering node's copy began: the offering node archives its copy). 409 when the name is a space of another person there.
- An archive: `{"space", "archived_at"}`. Answers `{"result": "archived" | "kept", "space"}`.

**Connections of peers.** A node that joins now gets a connection with `all` (it reaches every space of the person, present and future); the join answer's `spaces` lists the spaces the person has at that moment. A `peer-…` connection made by an earlier join (a read-write grant on « À trier » and on at least one other space, no path limits) is read as `all` too. A device paired to capture keeps its grant on the box alone.

**Filing from « À trier».** The note a `triage_send` (or the janitor) writes in the target space has an id derived from the note and the space (`f_t` and 20 hex characters, `memcp_core::triage::filed_id`), the same on every node.

**Rust.** `memcp_core::swarm` (`Offer`, `Answer`, `offers`, `accept`, `adopt`, `archive_here`, `archived_at`, `tombstones`, `since`); `memcp_client::peer_sync::Pass` gains `offered` and `archived`; `Peer::with_person`. **Command.** `memcp-sync --person ID`; its line ends with `N offered, N archived`.

## The capture program is gone: the node does its work (next version)

There is one program on every machine, the node (`memcp`, `memcp-manager`). It reads the voice notes, watches the folders and absorbs
the notes folders itself ([INTAKE.md](INTAKE.md)). The words *bridge* and *capture program* are gone from the code, the page and the
documents.

### Names that moved

| Before | Now | How the old one is treated |
|---|---|---|
| control database table `bridge_pairings` | `pairings` | migration 22 renames it, every row kept (an old database upgrades by itself) |
| `vaults` in the answer of `POST /pair` | `spaces` | `memcp_client::pair`, the Swift `pair(...)` read both; `Pairing.vaults` stays in Swift, deprecated, equal to `spaces` |
| `vaults` in the answer of the join state (`GET /join/<id>`, `join.vaults`) | `spaces` | not read back by any program of this repository (`memcp-sync` takes `mcp_url`, `key`, `agent`): change it if you read it |
| `vaults` in the answer of the `tidy` tool | `spaces` | not read back anywhere: change it |
| `vaults` in `GET /admin/api/timeline` | `spaces` | the page reads the new one |
| `vaults`, `vault_names` in the people list of `memcp-manager` | `spaces`, `space_names` | the page reads `spaces`, and `space_names` or the old `vault_names` |
| export manifest key `vaults`, folder `vaults/` | `spaces`, `spaces/` | `verify-export`, `adopt` and `merge` read both: an export made before still verifies and is taken in whole (`memcp_core::export::manifest_spaces`, `space_file`) |
| connection `desktop-bridge` (installers) | `desktop` | an installation that has `desktop-bridge` keeps it (its address is its name); the page shows a connection named `bridge-...` without the prefix, its name and address unchanged |
| `bridge.conf`, `bridge.key`, `device.key`, the `bridge/` folder (installers) | `memcp.toml`, `keys\desktop.key`, `keys\device.key` (Windows), the settings in `memcp.toml` | the installers carry the voice settings and the keys over once and keep the old folder as `bridge.migrated` |
| Keychain service `memcp-capture` (installers) | `memcp` | the installer moves the entries it finds |
| `MEMCP_BRIDGE_PORT`, `--bridge-port`, `-BridgePort` | gone (the node serves assistants itself, on its own port) | read, said (`no longer used`), ignored, for one release |
| `MEMCP_BRIDGE_BIN` (the program `/discover` asked) | gone (the node browses itself) | ignored |
| `Paired.vaults`, `Pairing.vaults` in Rust (`memcp_client`, `memcp_core::pairing`) | `spaces` | rename the field |

### What was removed

- The `memcp-capture` program, its release assets (`memcp-capture-<tag>-<target>`), its package entries, its LaunchAgent and
  Task Scheduler tasks (`org.memcp.bridge`, `org.memcp.transcribe`, `MemCP bridge`, `MemCP transcribe`: the installers remove them
  at the next run) and the systemd units `memcp-transcribe.service` and `memcp-transcribe.timer`.
  `memcp prepare transcribe` is kept for one release and does nothing (it removes the old settings file).
- The offline gateway and relay: the outbox, the local MCP on `127.0.0.1:8792`, the Unix socket, `memcp-capture connect`. A node is
  local: an app embeds it and writes into it. An assistant of the machine reaches the node at its own address with the key of its
  connection (`desktop`), or through `memcp mcp` (standard input and output).
- `memcp-capture discover` and `join`: `memcp intake discover`, and the page or `memcp-sync --join` to join a memory.
- From `memcp-capture-ffi` (the apps no longer use them): `Intake`, `IntakeConfig`, `IntakeItem`, `IntakeFlush`, `ItemKind`,
  `ItemState`, `LocalMcp`, `connectionOutbox`, `adoptOldCaptures`, `HeardInput`, `LiveLineInput`, `MarkdownPart`, `markdownNote`,
  `markdownFile`, `markdownMeeting`, `ReplicaWindow`, `replicaWindow`, `setReplicaWindow`, `pruneReplicaWindow`, `ReplicaError`.

### What stays in `memcp-capture-ffi`, unchanged

`pair`, `Pairing` (with `spaces` added next to `vaults`), `PairError`, `addressAllowed`, `BrowserLink`, `BrowserCall`,
`browserExtensionId`, `IntakeError` (what `BrowserLink` and `transcribe` throw), `transcribe(model:pcm:lexicon:)` (macOS),
`shouldTranscribe`, `summaryInstructions`, `summaryPrompts`, `summaryJoinPrompt`. They are deprecated: the node library is the place
they live now (below), and the app's own node crate can call them directly.

### What is new

**Settings** (`memcp.toml`, and in the Home Assistant app `intake_person` and `watch`; [PARITY.md](PARITY.md)): `intake_person`,
`transcribe_model`, `transcribe_cmd`, `transcribe_ffmpeg`, `transcribe_threads`, `transcribe_lexicon`, `transcribe_every`, `watch`,
`watch_every`, `absorb`, `absorb_exclude`, `absorb_protect`, `absorb_roles`, `absorb_every`, `deliver`. `transcribe` now needs
`transcribe_api_url` **or** `transcribe_model`; `transcribe_spaces` empty means every space the person writes to (it meant the
to-sort box on Linux).

**Commands**: `memcp transcribe`, `memcp watch [--once]`, `memcp intake send|text|absorb|status|discover`.

**Routes**: none. `GET /discover` answers as before, from inside the node.

**JSON forms**:

```jsonc
// POST /pair  ->  200
{ "agent": "peer-mac-ab12", "key": "hcp_...", "mcp_url": "https://node/mcp/peer-mac-ab12", "spaces": ["sam-a-trier"] }
// the join state, when approved
{ "state": "approved", "join": { "agent": "...", "key": "...", "mcp_url": "...", "spaces": ["..."] } }
// the tidy tool
{ "janitor_woken": true, "spaces": { "health": { "inbox": 1, "oldest_in_inbox": "...", "expired": 0 } }, "message": "..." }
// GET /admin/api/timeline
{ "hours": 24, "from": "...", "to": "...", "bucket_secs": 3600, "spaces": [ { "name": "work", "counts": [0, 2, 1] } ] }
// export manifest.json
{ "format": "memcp-export", "spaces": [ { "name": "sam-work", "kind": "work", "facts": 12, "files": 1 } ], "hashes": { "spaces/sam-work.sqlite": "..." } }
```

**Rust** (`memcp-server`, module `intake`; the app's node crate calls these):

| Name | Type |
|---|---|
| `intake::Settings`, `Settings::from_file(&config::Settings, state_dir)` | what the intake is told; `from_file` reads the options above |
| `intake::start(&Brain, Settings) -> Result<Option<Running>, Failure>`, `Running::stop()` | the background tasks of a node |
| `intake::Intake::new(&Brain, Settings)`, `.transcribe()`, `.watch()`, `.absorb(everything)`, `.waiting()` | one pass, by hand |
| `intake::Memory::open(&Brain, Option<&str>)`, `.call(tool, json)`, `.file_put(...)`, `.file_get(id)` | the node's hand on its own memory, with the rights of the person |
| `intake::whisper::VoiceSettings` (with `decoder: Option<Decoder>`), `Decoder(Arc<dyn Fn(&Path) -> Result<Vec<f32>, String>>)` | how recordings are read; with a decoder and `local-whisper` the model runs in the process |
| `intake::local::transcribe(model, pcm, lang, threads, lexicon)`, `transcribe_samples(...)` (feature `local-whisper`; `metal` on a Mac) | the speech model inside the program |
| `intake::browser::BrowserLink`, `EXTENSION_ID`, `EXTENSION_PORT` | the browser extension's link |
| `intake::plan::{should_transcribe, SUMMARY_INSTRUCTIONS, summary_pieces, summary_prompt, summary_join_prompt}` | the battery rule and the summary prompts |
| `intake::discover::discover(Duration) -> Vec<Found>` | nodes on this network and on the tailnet |

**Swift** (`memcp-capture-ffi`, `NodeIntake`): `NodeIntake(dataDir:settings:decoder:)`, `start()`, `stop()`, `transcribeNow()`,
`watchNow()`, `recordingsWaiting()`; the records `NodeIntakeSettings`, `NodeWatchedFolder`, `NodeVoiceReport`, `NodeWatchReport`,
`NodeRecordingsWaiting`; the protocol `NodeAudioDecoder { func decode(path:) throws -> [Float] }`; the error `NodeIntakeError`
(`invalid`, `failed`). Types and meanings: [INTAKE.md](INTAKE.md#the-apps).

## 20261117.0.16

- **`triage_send`** and the page's « send »: the note gets the automatic tags of the space it goes to, as `remember` gives them.

## 20261117.0.14

- **MCP `tags_suggest`** (read) `{space, text}` -> `{space, tags, fields, field_tags, all_tags, route: [{space, score}]}`: what the tagger would give for a text, without writing.
- **MCP `tags_set`** (write) `{id, tags? | add?, remove?, fields?, reason?, learn?}` -> `{result: "superseded", fact, added, removed, reason, learned}` or `{result: "unchanged"}`: corrects or completes the tags of a note (a new version; the old one stays in the history; a note that describes a file is refused; a note keeps at least one tag). Used by a device engine (Apple Intelligence through MCP) to correct or complete the tags after the note.
- **Automatic tags on write** (`remember`, each `deposit` record, the description of `file_put`): the node adds the tag of the space first, then the tags proper to it (keywords, vocabulary terms; in a health space the built-in signs and measures). The caller's tags are kept; none is removed. Not applied in À trier, for a key limited to paths, or for plumbing tags (`rules`, `index`, `living/…`, `janitor/…`), and not on `supersede`.
- **Fields**: a level or a measure said in the text is a field, stored as a tag `field/<name>/<value>` (`/` in the value is a path level, the decimal point is `_`: `field/level/2/10`, `field/tension/12/8`, `field/poids/72_5`). Every fact returned by the tools now has `"fields": {name: value}` read back from those tags (`recall`, `read_fact`, `living_get` cards and delta, the journal rows).
- **Learning**: a change of tags (page, `tags_set`, `supersede` with different tags) or a send from À trier teaches the node; two corrections make a word sure. `learn: false` on `tags_set` keeps a correction from teaching.
- **MCP `living_get`**: `first_pass: true` while no pass was ever recorded (then `behind` counts every note of the space). `living_set`: a pass that does not say `covers` has read up to the moment it writes.
