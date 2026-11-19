# API changes, for the apps

Every new route, tool or JSON form, with the version that brings it. The apps read this file. Where a name changed, the node reads both and writes the new one, unless it says otherwise.

## Big moves of notes, `supersede` keeps the kind (Unreleased)

- **`notes_move`** takes `limit` (1 to 5000, default 1000) and stops after about 20 s: call again until `remaining` is 0. The answer adds `already` (notes moved before by another call or a call cut off; never an error), `resumed` (notes of an interrupted move finished first), `failures` (`[{id, reason}]`, the first 10) and `remaining`. `matched`, `moved`, `left`, `failed`, `sample` keep their meaning, except that `failed` notes are no longer « a near duplicate »: a move copy is never held back by its neighbours, and a refusal now carries its reason. The copy of a note gets an id derived from the note and the target (the one `triage_send` gives), so a note moved twice, or by two nodes, is one note. A move is crash-safe: see TOOLS.md, `notes_move`. `POST /admin/api/space/move-tag` answers the same fields (plus `from`, `to`, `tag`); the page repeats the call until `remaining` is 0.
- **`supersede`** without `kind` keeps the kind of the replaced fact (it was reset to `fact`). The same text with another kind is accepted (it was refused as identical). New optional `add_tags`: tags added to the kept ones; `tags` still replaces them all.
- A space database gets a table `move_pending` the first time it moves notes (no schema version change: older programs ignore it).

## Journal handoff reminder (20261117.0.28)

Reads by assistants can include an `assistant_instructions` string at the top level of the tool payload, separate from stored records. It lists the whole, non-Inbox spaces involved in the read, with the connection's write rights. It asks for one dated handoff per substantive topic, covering the reasoning across multiple messages. Clients can ignore the new field without losing any data.

## OAuth for ChatGPT and Le Chat (20261117.0.27)

- `GET /.well-known/oauth-authorization-server` adds `authorization_response_iss_parameter_supported: true`: every redirect of `/oauth/authorize` (a code, `access_denied`, `invalid_request`) carries `iss`, the node's public address, exactly as the metadata's `issuer` (RFC 9207).
- `POST /oauth/register` accepts by default, besides Claude's callbacks: `https://chatgpt.com/connector_platform_oauth_redirect`, `https://chatgpt.com/connector/oauth/<id>` (an id of letters, digits, `-` and `_`, up to 128) and `https://callback.mistral.ai/v1/integrations_auth/oauth2_callback`. `oauth_redirect` still adds others. The consent page's `form-action` lists their origins.

## Files in chunks, up to 2 GiB (20261117.0.27)

A file over 24 MiB travels between nodes in chunks of 4 MiB. Everything below is added; the protocol stays 1, and a node of
an earlier version keeps exchanging files up to 24 MiB whole. The rules are in
[FEDERATION.md](FEDERATION.md#files-in-chunks-as-built).

- **Limits.** A space keeps a file of up to 2 GiB (`files::MAX_BYTES`, was 24 MiB). A file that comes in one request (`file_put`,
  a file put on the page) or from a space's folder stays at most 24 MiB (`files::MAX_INLINE`): above, `invalid` (`the file is
  over 24 MB, the most a file may be when it comes in one piece; ...`). A watched folder and `memcp intake send` take up to
  2 GiB, read a block at a time. `file_get` refuses a file over 24 MiB (`invalid`, `... too big to read in one piece`); the
  page serves it whole (`GET /admin/api/file`).
- **`GET /sync/spaces`** carries `chunks: 4194304` (the size of a chunk): this node takes and gives big files in chunks.
- **`GET /sync/events`** takes `chunks=1`: the `file_put` of a file over 24 MiB comes as `{..., sha256, size, chunk_size:
  4194304, chunks: [digest, ...]}`, without `data`. Without it, such a file and its retirement are left out (an erased one
  still goes, as its record). Once the same machine asks with `chunks=1`, the node goes back to the first event it left out.
- **`POST /sync/events`** takes the same form, and answers `wants: {file, missing: [{n, hash}]}` when it stopped on a file
  whose chunks it lacks (`stopped` says it too): the sender gives them, then sends the rest again.
- **`GET /sync/chunk?space=S&file=SHA&n=N[&hash=H]`**: chunk `n` of a file of the space, raw bytes
  (`application/octet-stream`), 404 when this node has neither the file nor that chunk. 403 for a space this node does not
  give the machine.
- **`POST /sync/chunk?space=S&file=SHA&hash=H`** (raw bytes, at most 4 MiB): a chunk of a file this node waits for; `{stored:
  true}`, 400 when the bytes do not match `hash` or no event asked for that file, 403 for a space this node does not take.
- **The pass** counts `chunks_in` and `chunks_out`, and says `SPACE: push: N file(s) over 24 MB not sent: the peer runs an
  earlier version, which takes no chunks` and `SPACE: pull: chunk N of file SHA refused: ...`.
- **On disk**: the chunks received of a file not yet whole wait in `sync-chunks/<space>/<sha256>/<chunk sha256>`.
- **Core**: `files::put_path`, `files::path_of`, `files::blob_path`, `files::MAX_INLINE`, the module `chunks`,
  `sync::events_after_with`, `Page::withheld`, `Applied::wants`, `sync::note_held`, `note_withheld`, `take_withheld`;
  `tools::file_put_path`.

## Sync policy per space and per machine (20261117.0.27)

What a node does with each space of the person and each of their machines. Everything below is added; nothing is
renamed. The rules are in [FEDERATION.md](FEDERATION.md#the-sync-policy-as-built). With nothing set, every space goes
both ways, as before.

- **Control migration 26**: table `sync_policy (peer, space, policy, set_at, theirs, theirs_at)`; `space = '*'` holds the
  choice for the spaces made later. No row means `both`.
- **`GET /admin/api/sync/policy?peer=PEER`**: `{peer, later, later_theirs, spaces: [{space, label, kind, parent, policy, set,
  theirs, sensitive, exchanged}]}`. `policy` and `later` are `both`, `send`, `receive` or `off`; `set` says the space has a
  choice of its own; `theirs` what the machine told of its side (or `null`); `sensitive` a space of health or money;
  `exchanged` the two exchanged it before (an `off` erases nothing). A machine that is not the person's: 403.
- **`POST /admin/api/sync/policy`** `{peer, space, policy}` or `{peer, later}`: sets it and answers the same view. Another
  person's machine or space: 403; a word that is not a policy: 400. Audited as `sync_policy`.
- **`GET /sync/spaces`**: a space set `off` for the asking machine is not listed; each listed space carries `policy` (this node's
  side) and `writable` is `false` when this node does not take it; the answer carries `later`. `archived` leaves out the spaces
  this node does not give.
- **`POST /sync/spaces`**: an offer of a space this node does not take is answered `{result: "declined", space, why}` (200); an
  archive told of such a space `{result: "kept"}`.
- **`GET /sync/events`** answers 403 for a space this node does not give the machine, **`POST /sync/events`** 403 for one it does
  not take.
- **`POST /sync/peers`** may carry `policy: {space: word, "*": word}`: the sender's side, kept for the page only.
- **Command line**: `memcp node policy PEER [SPACE POLICY | --later POLICY]`.

## Disputes (20261117.0.26)

A fact changed differently on two nodes is shown and resolved by the person. Everything below is added; nothing is renamed.
The rules are in [FEDERATION.md](FEDERATION.md#conflicts) and [TOOLS.md](TOOLS.md#disputes).

- **MCP `disputes`** (read) `{space?}`: `{notice, message, count, disputes: [{space, fact, since, alternatives: [{id, kind:
  "replacement"|"retired", current, text, author, at, reason}]}]}`. Not shown to a key limited to paths.
- **MCP `dispute_resolve`** (write) `{id, keep, space?, confirmed_by_person?}`: `keep` is an alternative's id (or its newest
  version) or `retired`; `id` the fact in dispute or one of its alternatives. Answers `{result: "resolved", space, fact, kept,
  retired: [id], message}`. An assistant must pass `confirmed_by_person: true` (else `denied`); a peer connection never sees the
  tool. A `keep` that is not an alternative, or a fact not in dispute: `invalid`, nothing changed.
- **`recall` and `read_fact`**: a fact that takes part in a dispute carries `disputed: true`, `dispute_of` (the fact in dispute)
  and `alternatives` (ids). Facts outside a dispute carry none of these fields.
- **`GET /admin/api/disputes`**: the `disputes` answer for the signed-in person's own spaces (`{count: 0, disputes: []}` when
  they own none). **`POST /admin/api/disputes/resolve`** `{id, keep, space?}`: the person's choice, as `dispute_resolve` from
  their page (no confirmation field needed); 400 with the reason when refused.
- **`GET /sync/status`** and **`GET /admin/api/sync/status`** carry `disputes`: how many disputes hold in the person's spaces.
- **A new event kind between peers, `dispute_resolved`** (eid `d:<fact>:<hlc>`, payload `{fact, kept, actor, at}`), logged after
  the `fact_retired` events of the versions that lost. A node of an earlier version refuses it as an unknown kind and keeps it out;
  the retirements still reach it. Update every node, or a resolution that keeps a replacement over a retirement is not seen there.

## Guided transcription (next version)

The node tells the speech model what to expect. Everything below is added; nothing is renamed. The rules are in
[INTAKE.md](INTAKE.md#guided-transcription).

- **MCP `transcription_hints`** (read, [TOOLS.md](TOOLS.md#transcription_hints)) `{space?, limit?, lang?}`: `limit` 1 to 500 (default
  120), `lang` `fr` or `en` (the language of the health terms; another or none: both). Not shown to a key limited to paths. A space
  the caller does not read: `denied`. Without `space`, the spaces the caller reads, and only those.
- **`GET /admin/api/transcription/hints?space=&limit=&lang=`**: the same answer for the signed-in person (the spaces they read); a
  space they do not read answers 400.

```json
{
  "words": ["Quentin Martin", "Lisbonne", "Sophie", "Zendure", "chaudière", "devis", "plombier", "céphalée"],
  "text": "Quentin Martin, Lisbonne, Sophie, Zendure, chaudière, devis, plombier, céphalée",
  "groups": {
    "names": ["Quentin Martin", "Lisbonne", "Sophie", "Zendure"],
    "keywords": ["chaudière", "devis"],
    "vocabulary": ["plombier"],
    "health": ["céphalée"]
  },
  "built_at": "2026-11-17T10:00:00Z",
  "version": 4821907345211,
  "space": null,
  "lang": "fr"
}
```

`words` is the four groups one after the other (names, keywords, vocabulary, health), at most `limit` items of 40 characters or
less, none twice whatever the case; `text` is the first of them that fit in about 220 tokens, comma separated, for an initial prompt
(Whisper) or `contextualStrings` (Apple speech: give `words`); `built_at` is when the oldest list it is made of was worked out;
`version` is a whole number (under 2^53) that changes when the notes, rules or vocabularies it comes from changed.

- **Setting `transcribe_hints`** (`memcp.toml` and the Home Assistant app's options, default `true`): the node's own transcription
  passes `text` to `whisper-cli` (`--prompt`, after the terms of `transcribe_lexicon`), to the speech model inside the program (its
  initial prompt) and to a speech service of the OpenAI kind (the `prompt` field of the request). With a service, those words leave
  the machine with the recording; `false` sends the recording alone (the lexicon still goes as before).
- **Rust**: `memcp_core::transcription::{hints, Hints, Groups, builds}`; `intake::whisper::VoiceSettings.hints: bool`,
  `Whisper::transcribe_with(audio, work, hints)`, `join_prompt`; `intake::local::transcribe_samples_with(..., hints)`.

## The health referential, in the node (next version)

The node's tagger no longer has its own hardcoded health patterns: it reads the health referential ([HEALTH.md](HEALTH.md)), a library of its own (`crates/health`) with embedded data files, versioned. Everything below is added; nothing is renamed, and every tag the first tagger gave is still given.

**MCP tool** ([TOOLS.md](TOOLS.md)).

- `health_referential {kind?, query?, lang?}`: read on at least one space. The signs (85), the measures (7), the vital constants (9) and the red flags (15), compact, with the version of each part. `kind` is `manifestations`, `measures`, `vitals` or `red_flags`; `query` keeps the entries whose id, label or a keyword holds those words; `lang` is `auto`, `fr`, `en` or `es`.

```json
{ "notice": "…", "lang": "auto", "versions": { "manifestations": 1, "measures": 1, "vitals": 1, "red_flags": 1 },
  "measures": [{ "id": "blood_pressure", "unit": "mmHg", "labels": { "fr": "Tension artérielle", "en": "Blood pressure", "es": "Presión arterial" },
                 "keywords": { "fr": ["tension", "pression arterielle", "ta"], "en": ["blood pressure", "bp"] },
                 "units": [{ "id": "mmhg", "label": "mmHg" }, { "id": "cmhg", "label": "cmHg" }], "vital": null }] }
```

**Tags and fields of `tags_suggest` and of every write in a health space** ([AUTOTAG.md](AUTOTAG.md)).

- `alerte/<id>` for a red flag (`alerte/chest_pain`), given first, and the field `red_flag` with the first one (`field/red_flag/chest_pain`). The 15 comes before the 3114. A red flag exists in French only; an app must not take the absence of an alert in another language for safety.
- `signe/<id>` for the 64 signs the first tagger did not have (`signe/loneliness`, `signe/shame`...); the 21 it had keep their French name (`signe/cephalee`, `signe/douleur`...). A sign denied (`pas de fièvre`) is no longer tagged.
- `mesure/sommeil` with the field `sommeil` (hours, `6.5`) and `sommeil_unit` = `h`, for `dormi 6h30`, `j'ai dormi 2 h`.
- The field `level` is also read when it is said after a comma (`anxieux, 2 sur 10`).

```json
{
  "space": "health",
  "tags": ["health", "alerte/chest_pain", "signe/douleur-thoracique", "mesure/tension"],
  "fields": { "red_flag": "chest_pain", "level": "6/10", "tension": "12/8" },
  "field_tags": ["field/red_flag/chest_pain", "field/level/6/10", "field/tension/12/8"]
}
```

The janitor's `#santé` rule adds the same `alerte/`, `signe/` and `mesure/` tags to the note it files in `health`.

## The exchange between nodes keeps what it cannot place (next version)

Everything is added; the protocol stays 1, and a node of an earlier version reads what it knows. See [FEDERATION.md](FEDERATION.md).

- **`POST /sync/events`** answers two more fields: `not_kept: [{eid, why}]`, the events dealt with but not kept (`why` is the reason of a refusal, `null` for one left out of the receiver's window), and `gave_up: [text]`, what waited too many exchanges for an event that never came and went on without it. A pushing node counts no event of `not_kept` as held by that peer, so its window never prunes it; against a node of an earlier version, a page with refusals (`malformed` above 0) counts as not held at all.
- **An event that needs one the receiver refused** is refused too (`why`: `it needs fact ID, which this node refused (...)`) instead of stopping the space for ever. One that stops ten exchanges on an event no peer sends goes on as if that note or file were outside the window.
- **`file_retired`** carries `erased: true` for a recording erased after its transcription (a node of an earlier version gives the reason `erased: ...`, read the same): every node then erases its copy of the bytes, and no node sends them again (its `file_put` travels with `erased: true` and no `data`).
- **A file is at most 24 MiB** (`files::MAX_BYTES`), whatever way it comes in (`file_put`, a watched folder, a space's folder): above, the answer is `invalid` (`the file is over 24 MB, ...`), and a folder's file is left alone. A file kept before this limit travels as its record alone. A page of events stays under its size unless it holds one event.
- **A layout move under a parent space this node does not hold** waits aside and applies at a later exchange (it was refused and lost).
- **A space of the peer named like a space of another person here** is never paired: the pass says `NAME is the name of a space of another person here: it is not exchanged`.
- **Archive times** told by a peer (`archived` of `GET /sync/spaces`, `archived_at` of `POST /sync/spaces`) are held back to five minutes past the receiver's clock; a space brought back counts as begun after the archive it comes back from.
- **The pass reports refusals on both sides** in its problems (`SPACE: pull: N event(s) refused here: ...`, `SPACE: push: N event(s) refused by the peer: ...`).
- **A merge** (and a space deleted into another) logs everything it writes, so the other nodes get the merged space; a note that gains tags in a merge is replaced by a copy of itself with every tag.
- Space migration 13 adds `sync_unheld`, `sync_rejected`, `sync_waiting` and `sync_deferred`. A received file is logged without its bytes (they live once, in the space's files).

## A machine is what made it, not its name (next version)

What a key may do no longer hangs on its name. The apps keep working: the Mac and the iPhone hold machine keys made by a pairing code or a join, and the Mac uses its console key for `/admin/api`.

- **Kinds of connection.** Control migration 25 adds `connections.kind`: `assistant`, `oauth`, `peer` or `console`, set by the code that makes the connection. A `peer` comes only from a pairing code or a join the person allowed, or from `memcp connect peer-…` on the node's own command line. Existing rows: `console` for the page's keys, `oauth` for those holding an OAuth refresh token, `peer` for those the node recorded making for a machine (the pairing code's row, or `paired` / `joined` in the audit, followed through renames), and for those that already synced as a machine: a `peer-…` name found in `sync_state` or as the `link` of a node of the peer directory (followed through renames), on a connection that reaches every space, whole, and may write. Every other row is an `assistant`, whatever its name; neither the name alone nor sync rows alone make a machine. **A machine whose connection the node cannot prove must join again.** In Rust, `access::Caller` gains `kind` and `local`, `access::ConnectionInfo` gains `kind`; `access::connect_peer` and `access::connect_oauth` are new; a `Caller` built by hand (as the apps' in-process node does) sets `kind` and `local` too.
- **`peer-` names are kept for machines.** `POST /admin/api/connections`, `connect_for`, a rename and an OAuth address whose name starts with `peer-` are refused (`400`, or `403` on the consent page). `memcp connect peer-NAME` on the node itself makes a machine connection (no `--days`, no `--read-only`).
- **`/mcp/<connection>/sync/*`**: `403` for any key that is not a machine's. `GET /sync/status`, `GET|POST /sync/peers` and `POST /sync/spaces` need a full peer (a machine that reaches every space of the person, whole, and may write); `GET /sync/spaces` and `GET|POST /sync/events` stay open to a machine limited to some spaces, on what it reaches. `offers` in `GET /sync/spaces` is `true` for a full peer only, as before.
- **`POST /mcp/<connection>/join/invite`** and **`/join/pending|approve|deny`** under a connection: a full peer only, by kind (a key named `peer-…` that is not a machine's is `403`).
- **`POST /admin/api/join/allow-device`** `{device, minutes?}` now answers `{result: "allowed", device, minutes, ticket, link, expires_in, single_use: true}`; `minutes` is 1 to 10 (default 10). The device joins with `POST /join {device, ticket}` (or `memcp-sync --join` with `MEMCP_JOIN_TICKET`). A request that only gives the device's name waits for the three words, like any other.
- **`POST /join`**: at most 3 requests waiting from one address (an IPv6 /64) and 64 in all (it was 8 in all); a request with a ticket is not counted. Over either cap: `429`.
- **OAuth**: a consent renews only a connection a consent made for the same person. When the name is held by any other connection, the client gets `<name>-2` (then `-3`…), and the other connection keeps its key and rights.
- **`folder_set`** is offered, and runs, only from this machine (the standard input of `memcp mcp`, or HTTP from the loopback address with no `X-Forwarded-For`, `Forwarded` or Tailscale header) and never to an OAuth connection; elsewhere it is an unknown tool. The page (`/admin/api/space/folder`) and the command line are unchanged. A folder that is, is inside or holds a folder of the system or of keys, and a path with `..`, are `invalid`, checked before anything is made on the disk.

## The command line reaches a running node (next version)

Every command of `memcp` also runs against a node that is up, with a key ([CLI.md](CLI.md)). All routes are under `/admin/api`, authorised by a console key (`Authorization: Bearer hcpc_...`), with `X-Requested-With: memcp` on a POST, and audited.

- **`POST /admin/api/cli`** `{ "argv": ["space", "new", "Cars"] }` runs a command of the command line for the key's person, on the node's data, and answers `{ "code": 0, "stdout": "...", "stderr": "..." }`: exactly the text and exit code of the local command (`code` 1 for a failure of the command, 2 for a wrong command line). A command of the machine itself (`init`, `serve`...) answers 400 with the reason; a command that needs an administrator, 403 with a sentence; a command a member runs on another person (`--user`), 403. An administrator acts for anyone with `--user`; any other key acts for its own person. The files a command reads must have been sent first (`@upload:ID[/path]` words).
- **`POST /admin/api/upload?id=&path=&offset=`** takes one chunk of one file (raw bytes, up to 32 MB; the client sends 8 MiB): `id` is 8 to 32 letters or digits, `path` a plain relative path, `offset` the size of the file now (409 otherwise: a chunk is never written twice). 8 GiB at most per `id`, 200 000 files; a folder not used within 12 hours is removed.
- **`POST /admin/api/upload/discard`** `{ "id" }` forgets a folder that was sent.
- **`GET /admin/api/export`** now sends the tar from a file, block by block, instead of holding it in memory; the answer is the same.
- **`GET` and `POST /admin/api/space/folder`** are named by the command `memcp folder get|set` and are **pending** (they come with the space folders): the command answers exit 6, "pending", on a 404.
- Command line only: `memcp status`, `memcp person new`, `memcp note recall`, `memcp note tags`, `memcp connection list|new|regrant|revoke|internal|readonly|person-guide|limit` (the former words still work), `memcp invite code|ticket|pending|approve|deny|allow` and `memcp join LINK|ID|ADDRESS [--ticket T]` (they call `POST /pairing`, `POST /nodes/invite`, `GET /join/pending`, `POST /join/approve|deny|allow-device`, `POST /nodes/add`, `GET /join-memory`), `memcp folder ...`. `--prefix` and `--token-env` are read as options with a value (they were read as switches, so `adopt DIR --prefix P` did not apply the prefix).
## Call recordings carry their participants (next version)

Everything below is added; nothing is renamed. `fields` of a fact keeps its form for the scalar fields, and gives a list for a name
held more than once (and always for `caller` and `callee`).

**`call`** (optional) on `remember`, on each record of `deposit`, and on `file_put`: `{ "caller": [..], "callee": [..], "app": "phone|facetime|whatsapp|teams|meet|zoom|other", "direction"?: "in|out|missed", "started_at"?: ISO 8601 or Unix seconds, "duration_s"?: integer }`. At most 20 participants (both sides), 80 characters each, no control character; `caller`, `callee` and `app` required, at least one participant; otherwise `invalid` and nothing is written. A name is kept as a slug (`Alice Martin`: `alice-martin`; `/` is a path level), a phone number as its digits, returned with a leading `+`.

An iPhone call recording:

```json
{
  "name": "file_put",
  "arguments": {
    "space": "personal",
    "name": "2026-11-17-09h12-call.m4a",
    "note": "Call recording",
    "tags": ["inbox/audio"],
    "data_base64": "AAAAGGZ0eXBNNEEg...",
    "call": {
      "caller": ["Alice Martin"],
      "callee": ["+33 6 12 34 56 78"],
      "app": "phone",
      "direction": "in",
      "started_at": "2026-11-17T09:12:00+01:00",
      "duration_s": 754
    }
  }
}
```

The description fact then reads:

```json
{
  "id": "f_c1", "type": "file",
  "tags": ["appel", "appel/phone", "field/call_app/phone", "field/call_direction/in", "field/call_duration_s/754",
           "field/call_started_at/1794903120", "field/callee/33612345678", "field/caller/alice-martin",
           "inbox/audio", "personal", "personnes/alice-martin"],
  "fields": { "call_app": "phone", "call_direction": "in", "call_duration_s": "754", "call_started_at": "1794903120",
              "callee": ["+33612345678"], "caller": ["alice-martin"] }
}
```

A Teams meeting with three people, as a note:

```json
{
  "name": "remember",
  "arguments": {
    "space": "work",
    "text": "2026-11-17, weekly sync: budget approved.",
    "call": { "caller": ["Alice Martin"], "callee": ["Bob", "Carol Durand"], "app": "teams" }
  }
}
```

```json
{
  "result": "created",
  "fact": {
    "id": "f_c2",
    "tags": ["appel", "appel/teams", "field/call_app/teams", "field/callee/bob", "field/callee/carol-durand",
             "field/caller/alice-martin", "personnes/alice-martin", "personnes/bob", "personnes/carol-durand", "work"],
    "fields": { "call_app": "teams", "callee": ["bob", "carol-durand"], "caller": ["alice-martin"] }
  }
}
```

When the node transcribes a recording that has a call, the transcription note (tags `inbox`, `dictee`) **inherits** the same `field/...`, `personnes/...`, `appel` tags. The janitor keeps them when it files the note.

**`tags_set`**: `fields` takes a list for `caller` and `callee` (the whole side is replaced; `[]` empties it); `add`/`remove` of `field/caller/<name>` and `field/callee/<name>` keep the `personnes/<name>` tags in step. **`tags_suggest`**: new optional `tags` (the note's tags); the `personnes/` ones make `route` answer `{ "space", "score", "by": "people" }` where earlier notes with that person went (learned when the person sends a note of Inbox to a space).

**Core**: `memcp_core::call::people_names(brain, space?)` returns the distinct names of the `personnes/` tags with their counts, most first; `memcp_core::autotag::fields_json(tags)` is the JSON form of the fields.

## A folder for each space, on each node (next version)

Everything below is added; nothing is renamed. The rules are in [FOLDERS.md](FOLDERS.md).

**MCP tools** ([TOOLS.md](TOOLS.md)).

- `folder_get {space?}`: read on the space (whole, not by paths). With `space`, the object below; without, `{ "folders": [object...] }` for the readable spaces that have a folder.
- `folder_set {space?, path, mode?}`: write on the whole space. `path` is the full path on this node, or `null` for the node's own storage folder; `mode` is `readwrite` (default) or `read`. Answers the same object. Errors: `invalid` (relative path, a file, inside the data directory or overlapping another space's folder, unknown mode), `denied`.

```json
{ "space": "home", "path": "/home/me/Notes/home", "default_path": "/var/lib/memcp/files/home", "custom": true,
  "mode": "readwrite", "cloud": null, "warning": null, "last_scan": "2026-11-17T10:00:00Z", "files": 12, "errors": [] }
```

`cloud` is `"google-drive"`, `"icloud"`, `"onedrive"`, `"dropbox"` or `null`; `warning` is a text (advice to keep the folder on the device) or `null`; `mode` is `null` and `custom` is `false` when no folder was chosen (`path` is then `default_path`, and nothing is mirrored). The setting is **per node**: it is in no sync event and does not travel in the swarm. An app shows `warning` next to the folder, in its own words.

**Node page routes** (`/admin/api`, console token).

- `GET /space/folder?space=NAME`: the object above. 403 for a space that is not the person's.
- `POST /space/folder`: `{"space", "path", "mode"?}` sets the folder (`path` `null` or empty: back to the default; naming a path is for administrators, 403 otherwise), answering `{"ok": true, "folder": {...}}`; `{"space", "op": "sync"}` makes a pass now, answering `{"ok": true, "scan": {"written", "edited", "new_notes", "new_files", "conflicts", "retired", "waiting", "errors"}, "folder": {...}}`. Both take `"dry_run": true` and answer `{"dry_run": true, "would": "…"}`. Audit event: `folder_edit`.
- `POST /space/library` `{op: "add"}` is kept: it is `POST /space/folder` with `mode: "read"` (it replaces the folder of the space on this node); `{op: "remove", id}` also puts the space back on its default folder when the root removed was its folder. `GET /space/libraries` still lists the index roots.

**Command line.** `memcp folder set SPACE PATH [--read]`, `memcp folder set SPACE --default`, `memcp folder get [SPACE]`, `memcp folder sync [SPACE]`. `memcp library add PATH --space NAME` stays, as an alias of `folder set NAME PATH --read`. Environment: `MEMCP_FOLDER_EVERY` (seconds between two passes while serving, default 10).

**What the node writes into the memory.** Author `folder-sync`. A note edited in the folder is a `supersede` with the reason `edited in the folder on this node`; a deleted file is a `retire` with the reason `its file was deleted from the folder on this node`; a note changed on both sides keeps both, the folder's copy being a new note tagged `conflict`; a new file or `.md` is a file or a note. The Markdown front matter keys are `id`, `space`, `tags`, `created`, `version`.

**Storage.** Control database migration 24: `space_folders` (`space`, `path`, `mode`, `cloud`, `set_at`, `last_scan`, `last_files`, `last_errors`) and `folder_state` (what the node remembers of each file: hash, size, time, the note or file it stands for). Neither is in a space file or in the sync log.

**Rust.** `memcp_core::folders`, `memcp_server::folder_sync`; the embedded node of an app (`NodeIntake::start`) runs the task too.

## Adding a node by its id or link, from any node (next version)

Added only; nothing existing changes except one more key in two answers. Details and the rules: [FEDERATION.md](FEDERATION.md), *Adding a node*.

- **Node id**: random, `ABCD-EFGH-IJKL`, made once. It names a node and proves nothing. It is in `GET /.well-known/memcp` (`id`), in `node` of `GET /sync/status` and `GET /sync/peers` (`node.id`), in each directory entry on the wire (`id`) and in each row of `GET /sync/status` `peers` (`id`, `addresses` best first, `via`).
- **`GET /admin/api/node`**: `{ id, name, version, addresses, exposures, link, qr }`; `link` is `memcp://node?id=…&name=…&addr=…[&addr=…][&t=ticket]`, `qr` an SVG.
- **`POST /admin/api/nodes/add`** `{ "link": "<link | id | address>" }` (administrator): this node asks THAT node to join, with `POST /join`. Answers `{ "id": "<request>", "state": "asking", "node": { "id", "name" } }`; follow it with `GET /admin/api/join-memory?id=` (`asking`, `waiting` + `check`, `approved`, `denied`, `expired`, `failed` + `error`). Refused (400) with words: not a link, this node, already linked, not known (an id nobody told). The link's `t` is a one-time ticket: the node lets this one in at once, with no words. `POST /admin/api/join-memory` also accepts a `memcp://` link in `address`.
- **`POST /admin/api/nodes/invite`** `{}` → `{ "link", "qr", "expires_in": 600, "single_use": true }`; `POST /mcp/<peer>/join/invite` answers one more key, `link`.
- **`POST /admin/api/nodes/via`** `{ "name" | "id" | "peer", "address": "<one it told>" | null }` → `{ "ok": true, "name" }`: the address this node asks that node at first.
- **Requests on every peer**: `GET /sync/peers` → `node.pending: [{ id, device, check, node_id, waited, expires_in }]` (to a peer of the person only). A peer lists them in its `join/pending` (`GET /admin/api/join/pending`, `GET /mcp/<peer>/join/pending`) with `node` and `remote: true`; every request now carries `node_id` (the id of the asking node, when it says it, in `POST /join` `{ device, node_id?, ticket? }`). Answering a remote one (`POST …/join/approve|deny` `{ id }`) answers `{ "result", "via": "<node>", "queued": true }`; the answer goes to the asked node in `POST /sync/peers` → `answers: [{ id, result: "approved" | "denied" }]`.
- `memcp-sync --join ADDRESS [--join ADDRESS…] [--expect-id ID]` (ticket in `MEMCP_JOIN_TICKET`); the kept peer is now named after the node that answered (`peer-<node name>`). `MEMCP_JOIN_TTL` (seconds, 2 to 3600) shortens how long a request waits; the default stays 120.
- Migration 23 of the control database: `peer_directory.node_id`, table `remote_joins`.

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

## 20261117.0.18

- **MCP `notes_move`** (write) `{from, to, tag, dry_run?}` -> `{from, to, tag, dry_run, matched, moved, left, failed, sample}`: moves the notes under a tag from one space to another (copy, then the original is retired with the reason « moved to … »), as the page's « move this tag ». `dry_run` defaults to true. Write access on both spaces is needed.
- **MCP `space_create` / `space_move`**: documented in docs/TOOLS.md (they were already in 0.17).
- The keyword learning ignores a larger list of empty words (French, English, Spanish).
