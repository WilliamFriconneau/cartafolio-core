# Peers: every node is a copy, and together they are the backup

Status: plan. Built after the spaces (see SPACES.md), the mounted folders and the app.

## The idea

There is no central server. A MemCP **node** is a program with a local store and a list of peers: a server, a bridge on a laptop, a Windows PC, a phone. Nodes synchronise with each other the way torrent clients and Syncthing do: any node that has something gives it to any node that lacks it. A node that holds everything is a complete copy, so **synchronisation and backup are the same thing**: losing a machine loses nothing as long as one other node holds the memory.

A node can be switched off for days. When it comes back it asks its peers what it missed and sends what they missed. Nothing needs to be online at the same time and nothing needs a hub.

One node may be the **admin**: the one whose page the person uses to see all the nodes, to add or revoke one, and to set what each may hold. In a home installation that is the Home Assistant app. It is a role, not a different kind of program: any node can take it over.

## How a person grows into it

1. **One Windows PC.** The person installs the app (HealthApp). All the data is on this PC, in a folder an assistant such as Claude can read, with the instructions in it. No server, nothing else.
2. **A second computer** joins as a peer. They synchronise. An assistant runs on either. If one is lost, the other has everything.
3. **Their own server**, when they want one, is one more node (Linux, Windows or anything that runs the program). It is "the MemCP server" only because it is always on and reachable. It can rebuild every other node.
4. **Three nodes or more**, each synchronising with the others, whichever are on. A laptop closed for a week catches up when it opens.

The same program does all four steps: nothing is migrated from a "simple" version to a "server" version.

## Why it can work: facts are events

A fact is never overwritten. It has an id, it is added, and it is later replaced (`supersede`) or retired, and the history keeps everything. A space is therefore an **append-only log of events**. Merging two copies is the union of their events, in a deterministic order: no fact is lost, and the same events give the same result on every node, in any order of arrival. Real conflicts are few and have a known rule:

| What | Rule |
|---|---|
| A fact added on two nodes | Both are kept (ids differ). Duplicates are left to the janitor, as today. |
| The same fact replaced on two nodes | Both replacements are kept and the fact shows as *in dispute* until the person picks one. Never a silent winner. |
| A fact retired on one node, replaced on another | Shown as in dispute. |
| Layout of spaces (label, parent, position) | Last writer wins, per field, by a hybrid logical clock (never the wall clock alone). |
| Keywords, tags and routing rules | Shared by the whole swarm, as data with a version: each rule has an id, and the newest change of a rule wins by the logical clock. A node applies the rules it has; the result is the same on every node that has the same rules. |
| A note's *processed* mark | A replicated event: see *Processed notes* below. |
| Guides | One writer at a time: the person, on a node they sign in to. Others receive them. |
| Mounted folders | A mount is a logical folder; each node holds it at a path of its own. Per mount and per pair of nodes the person chooses: files and passages, passages only, or nothing (see SPACES.md, *One synchroniser per folder*). |
| Files (photos, recordings, documents) | Content-addressed by hash, in chunks, like a torrent: a file is fetched from any node that has it, once. |

## Shape

- **Peers are equal.** Which nodes talk to which is set by the person; the default is that each node talks to all the nodes it knows. A node behind a router connects out; nothing needs an open port when the transport can punch through or use a relay.
- **Per space and per node, a policy chosen by the person**: *do not sync*, *send*, *receive*, *both*. The default is *do not sync*. A sensitive space (health, money) never leaves a node unless the person turns it on, space by space and node by node, and the page says so.
- **Identity.** Each node has a key pair; a node is known by its public key. The person adds a node by confirming its key (a short code or a QR code shown on both screens). A lost or stolen node is revoked: the others stop sending it anything. What it already holds stays on it, so the page says which nodes hold what.
- **Transport.** Encrypted, between keys, with no dependence on addresses. The candidate is [iroh](https://github.com/n0-computer/iroh) (Rust, Apache-2.0 or MIT): QUIC connections made by public key, hole punching with relay fallback, and companion crates for content-addressed blobs and an eventually consistent store. Points to check before choosing it: a secondary source reports iroh 1.0 in June 2026, but the companion crates (`iroh-blobs`, `iroh-docs`) are reported as still before 1.0 and their README warns about production use; platform support (Windows, macOS, Linux, iOS) is not stated on the pages read. So the transport sits behind one small interface, the protocol of events above is MemCP's own, and the library is pinned to an exact version. Another transport (Syncthing's open protocol, a plain TLS connection to a known address) can replace it without touching the rest.
- **Catching up.** A node asks a peer "the events of this space after my last point for you", and keeps one point per peer per space, so a node that was away resumes where it stopped.
- **At rest.** Each node protects its own store as it does today. More nodes mean more places holding the memory: the page lists them, with what each holds and when it last synchronised.

## Processed notes: no loop in the swarm

Every node may run a janitor, and a note reaches all of them. Without care each would file the same note, again and again, and every copy would be replicated to the others.

- **A mark, not a memory.** When a janitor has filed a note, it writes a *processed* event: note id, the node, the version of the rules it used, the time. The mark replicates like any event. A note carrying a mark is never picked again by any janitor on any node.
- **Claim before work.** A janitor first writes a short *claim* on the note (a lease of a few minutes). Two claims on the same note: the older one by logical clock wins, the other node drops the note without filing it. A claim that expires without a mark is free again, so a node that dies midway loses nothing.
- **Reprocessing is a decision, not an accident.** A note is filed again only when the person asks, or when the rules changed *and* the person has chosen to refile what the old rules left in the inbox. The mark keeps the version of the rules, so "filed with an older version" can be listed.
- **Filing keeps the note.** The janitor files a note by `supersede` with the same text and its real tags, never by rewriting it (the existing rule), so the history shows what was done and by which node.

## Rules that improve themselves

An assistant (a model) may work on the keywords and tags: see which notes stay in the inbox, which were refiled by hand, and suggest a keyword for a space, a new tag, a merge of two tags. The limit is the rule that already holds: **what an assistant writes is data or a proposal, never an instruction**, and the router that applies the rules stays deterministic (a model never decides at the moment a note arrives, except as the janitor of what the keywords could not file).

The person sets, per space, how far an assistant may go:

| Level | What the assistant may do |
|---|---|
| *Propose* | Write proposals only; the person approves each one on the page or in the app. |
| *Add* (recommended default) | Add an ordinary keyword or a tag to a space that is not sensitive, and apply it at once. Everything else is a proposal. |
| *Full* | Also change and remove keywords, create sub-spaces. Never on a sensitive space. |

Whatever the level, these always wait for the person: a **unique** keyword (one that decides alone), anything on a sensitive space, a rule that sends notes outside the node (a drive, another server), and any removal on a space that has a connector. Every change is an event in the history with the name of the assistant and the reason it gave, can be reverted in one step, and is limited in number per day. A change that makes more notes leave the inbox but also moves notes the person had filed by hand is shown as such before it applies.

## A node that holds only part of the memory

A phone should not carry a whole memory. A node may be set to hold only a **window**: the spaces it needs, and in each the last N days (at least the widest window of a dashboard it shows), plus what the person pins. Everything else stays on the nodes that hold it and is fetched when asked, the way a drive streams a file.

- **Write first, prune later.** A note written on the phone is kept in its outbox and pushed to the other nodes or the drive connector. It leaves the phone's store only after **at least one other node has acknowledged it**: never before. A note that no node has acknowledged is never pruned.
- **How the window works** (`memcp_core::sync`). The setting is the replica's own and off by default: `memcp-sync --window-days N` (`off` to clear), or `set_window_days` (the apps, through the FFI crate: `set_replica_window`). The first catch-up takes only the events of notes created in the last N days, and the notes that are pinned (tag `pin`, or under `pin/`); open notes of boards and guides (`dashboard`, `rules`, `index`, `guide`, `living/…`) count as pinned. `prune_window(brain, space, days, now)` (run for every space after each pass) removes, physically, the notes older than the window in any status, with their tags, history, search entries, files, file bytes and log rows, but only the ones that are not pinned, that no kept note was replaced by, and whose events (creation, replacement, retirement, files) every peer this replica syncs with has acknowledged (it sent them, or this replica's push cursor is past them). With no peer, or a peer whose progress this replica cannot see, nothing is pruned. Notes waiting for approval stay.
- **Pruning is local.** It writes no event and tells no peer. A later event from a peer about a note that is gone here (replaced, retired, a file added) is ignored, not kept: this replica does not record the newer state of a note it does not hold, and does not wait for it. A new note that replaces a pruned one is kept, without the link to its predecessor. A pinned note replaced by a note outside the window is closed here as retired, with that reason. A node that joins later is fed those old notes by the nodes that hold everything, not by this replica; turning the window off does not bring them back.
- **Seeing it.** `GET /sync/status` and the `peers` tool carry `window` (`window: N days` or `window: all`, and what was pruned last: when, how many notes and files, how many kept for want of an acknowledgement).
- **Write-only connectors.** A connector may be *write only*: the node pushes to a drive folder (or a server) and keeps nothing of what it wrote beyond the window. It is the way to archive without filling the device.
- **Large raw data stays where it is big.** The raw records an app deposits (tens of thousands of health samples) are held by the always-on node; the phone holds the day-by-day summaries its dashboards read, and fetches raw records on demand.
- **Offline means the window.** Without a connection, a node answers from what it holds; the page says that older notes need a connection.
- **Sensitive spaces** are pruned the same way, and never pushed to a drive connector that is not locked (see ENCRYPTION.md).

## The list of peers is shared

Like the peers of a torrent swarm, every node knows every peer of the memory, not only the ones that joined it. Each node keeps a **peer directory** in its control database (`peer_directory`, migration 19) and writes its OWN entry (its node name, set with `--node-name` or made once and kept; the program version; the time it was last active; how it can be reached) at every synchronisation pass and whenever it serves a synchronisation request. Nodes pass directories to each other at each pass (`memcp_core::directory`).

- **Endpoints**, at the address of the connection, like the rest of the exchange: `GET /sync/peers` answers `{node, peers}`; `POST /sync/peers` takes `{node: {name, ...}, peers: [...]}` and merges it. A pass pulls the peer's directory, then pushes its own. A node without these routes (an older one) answers 404 and the pass goes on without it: the directory is never a reason for a pass to fail.
- **An entry** on the wire: `{name, last_seen (unix), version, first_seen, revoked, revoked_at, exposures: [{kind, address?}]}`. Locally a row also keeps `learned_from` (the node that told this one), `stamp` (`last_seen` and the node it comes from) and `link` (the connection that reaches it directly).
- **Merge.** Per name the entry with the newest `last_seen` wins, and brings its version and exposures with it. Nothing is made up: a time later than the receiver's clock (plus five minutes) is held back to it, and a node never takes an entry about itself from another. `first_seen` is the earliest told.
- **Never removed, except by revocation.** When the person revokes a peer's connection, the entry of that peer gets `revoked: true`. The flag sticks (a later sighting does not clear it) and spreads with the rest, so the peer disappears from every list. The row is kept so the flag keeps spreading. An entry not seen for 90 days is listed with `gone: true`, and kept.
- **Version.** `behind: true` when the version (`AAAAMMJJ.0.N`, compared number by number) is older than this node's. A version that cannot be read is never `behind`.
- **Exposures** say how a node can be reached: `loopback` (this machine only), `lan` (the local network), `tailnet`, `public` (funnel, the internet), each with its address when known. A node takes them from its settings (the address it listens on, `--public-url`, the tailnet login) when it starts, and tells them in its entry. A peer whose only exposures are `loopback` and `lan` has `only_local: true` and `note: "syncs_on_open"`: it syncs when it is opened, with the peers it can reach. A node whose exposures are unknown is not said to be local.
- **Assistant connections** (the ones that are not peers) are listed apart with how each reaches the node: `cloud` when its last requests came from an address on the internet (a service such as claude.ai), `local` when they came from this machine, the local network, the tailnet, or not at all (a program of the person's). `via` is the node that served them, the one holding the connection.

The answer of the `peers` tool and of `GET /sync/status` (the same rows):

```json
{
  "node": { "name": "bravo", "version": "20261117.0.7", "last_seen": 1794800000,
            "exposures": [{ "kind": "lan", "address": "10.0.0.14:8791" }], "only_local": true, "note": "syncs_on_open" },
  "peers": [
    { "name": "alpha", "peer": "peer-alpha-0001", "connection": "peer-alpha-0001", "direct": true,
      "version": "20261117.0.7", "last_seen": 1794800000, "first_seen": 1794700000, "learned_from": "alpha",
      "gone": false, "behind": false, "exposures": [], "only_local": false, "note": null,
      "direction": "idle", "done": 120, "total": 120, "percent": 100,
      "spaces": [{ "space": "sam-me", "done": 120, "total": 120, "percent": 100 }],
      "last_activity": 1794799990, "allowed": null, "created_at": null, "last_used_at": null },
    { "name": "charlie", "peer": "charlie", "connection": null, "direct": false,
      "version": "20261101.0.1", "last_seen": 1794790000, "first_seen": 1794700000, "learned_from": "alpha",
      "gone": false, "behind": true, "exposures": [{ "kind": "loopback" }], "only_local": true, "note": "syncs_on_open",
      "direction": null, "last_activity": null }
  ],
  "connections": [
    { "name": "claude-web", "kind": "cloud", "via": "bravo", "last_used_at": "2026-11-17T09:00:00Z" }
  ],
  "window": { }
}
```

For a peer that holds a connection on this node, `connection` is its name (`peer-…`), `allowed` (the `peers` tool only) says whether that connection is still allowed, and `created_at` / `last_used_at` are dates. The rows of direct peers come first, then the others, the most recently seen first. Revoked nodes are not listed.

## Adding a node: an id, a link, from any node

Like adding a device in Syncthing: one node is shown to another, once, and the two become peers. It is done from ANY node the person holds; no node is the one that invites.

- **The node id.** Each node makes a random id once (60 bits, `ABCD-EFGH-IJKL`: twelve characters of the base32 alphabet `A-Z 2-7`), keeps it in its control database (`meta`, key `node_id`) and tells it everywhere it speaks: `GET /.well-known/memcp` (`id`), its own directory entry (`id`, `peer_directory.node_id`, migration 22) and so to every peer. **The id names a node and proves nothing**: whoever knows it can say it, and this is not a key pair. What lets a node in is always a person's answer (three words) or a one-time ticket, never the id. What the id does give: the node that answers at an address must say the id the link names (`memcp-sync --join A --expect-id ID`), otherwise the request is not sent (`the node at that address is not the one the link names`).
- **The link.** `memcp://node?id=ABCD-EFGH-IJKL&name=alpha&addr=https%3A%2F%2F…&addr=http%3A%2F%2F192.168.1.5%3A8791[&t=TICKET]`, shown as text and as a QR code. Addresses come from the node's `--public-url` and its exposures (the internet, the tailnet, the local network, this machine, in that order). `GET /admin/api/node` answers it without a ticket; with `t`, the ticket is the one-time introducer ticket of `POST /join` (below). A bare id, a bare address and the older `memcp://join?server=…` are read too.
- **Add a node** (`POST /admin/api/nodes/add` `{"link": "<what was pasted or scanned>"}`, administrator): this node asks THAT node to let it in with the existing `POST /join` (three words, shown on both sides), through `memcp-sync --join`, trying the addresses in order until one answers. The answer is `{id, state: "asking", node: {id, name}}`; `GET /admin/api/join-memory?id=` follows it (`asking`, `waiting` with `check`, `approved`, `denied`, `expired`, `failed` with `error`). Approved, the asking node keeps the peer (the key stays in `peers/`, private) and syncs by itself, both ways. A bare id is looked up in the peer directory (a node a peer told about is added without typing an address); an id that is this node, or a node already linked, is refused with words.
- **No words.** `POST /admin/api/nodes/invite` `{}` on a node the person already holds (or `POST /mcp/<peer>/join/invite` with a peer's key, which now also answers `link`) makes a link with a one-time ticket (ten minutes, kept as a hash). Scanning it on the new node joins at once, with a key of its own, no words: the ticket is the introduction from a node the person already holds. A ticket works once; a spent one is refused (`403`), it never falls back to waiting for a person. The ticket travels to `memcp-sync` in the environment (`MEMCP_JOIN_TICKET`), never on a command line.
- **The same three words on both sides, and the person from any of their nodes.** The asked node shows the request (`GET /admin/api/join/pending`: `{id, device, check, node_id, waited, expires_in}`) and the asking node shows the same `check`. The asked node also tells its peers: its entry in `GET /sync/peers` carries `pending: [{id, device, check, node_id, expires_in}]` (only to a peer of the person: a key that reads and writes all their spaces). Each peer keeps them (`remote_joins`, migration 22) and lists them in its own `join/pending` with `node` (the node that was asked) and `remote: true`. Answering there (`POST /join/approve|deny` `{id}`, or the same by a peer's key under `/mcp/<peer>/join/`) clears it at once on that node and answers `{result, via, queued: true}`; the answer goes to the asked node with the next exchange (`POST /sync/peers` `{node, peers, answers: [{id, result: "approved"|"denied"}]}`), which applies it as the person's click and drops the request from its list. Answered on one node, gone on the others at their next exchange with it (the pass is every ten seconds). A request is shown to the DIRECT peers of the asked node, not to peers of peers.
- **A request expires** after two minutes (`MEMCP_JOIN_TTL` seconds, 2 to 3600, is for tests): the asking node reads `expired`, the list is empty everywhere, and answering it is `404`.
- **Introducer.** The directory gossips each node's `id` and addresses (`exposures`), so a node that joined only B learns A (name, id, addresses) from B, without typing anything. `GET /sync/status` rows carry `id`, `addresses` (best first) and `via`. Such a row has `direct: false`; the page offers **Add this node**, which is `nodes/add` with the id alone: the node asks A at the addresses it was told. **Reachable via**: `POST /admin/api/nodes/via` `{name | id | peer, address}` chooses, among the addresses a node told, the one this node asks it at first (`address: null` clears it); an address the node never told is refused. The choice is the order of asking when a node is added again; it does not move a connection that already exists.
- **Revocation** is unchanged: revoking a peer's connection flags its directory entry, the flag sticks and spreads to every node, and the revoked node is refused at its next contact.

```json
// POST /admin/api/nodes/add
{ "link": "memcp://node?id=ABCD-EFGH-IJKL&name=alpha&addr=https%3A%2F%2Falpha.example.net" }
// -> { "id": "p3q9x2…", "state": "asking", "node": { "id": "ABCD-EFGH-IJKL", "name": "alpha" } }

// GET /admin/api/node
{ "id": "ABCD-EFGH-IJKL", "name": "alpha", "version": "…", "addresses": ["https://alpha.example.net"],
  "exposures": [{ "kind": "public", "address": "https://alpha.example.net" }],
  "link": "memcp://node?id=…", "qr": "<svg …>" }

// POST /admin/api/nodes/invite {}  ->  a link with a ticket, valid once
{ "link": "memcp://node?id=…&t=…", "qr": "<svg …>", "expires_in": 600, "single_use": true }

// GET /admin/api/join/pending, on a peer of the asked node
{ "requests": [{ "id": "k3…", "device": "beta", "check": "amber-river-moss", "node": "alpha", "remote": true, "expires_in": 97 }] }
```

## What this is not

- Not a replacement for a file synchroniser: mounted folders can still be synchronised by Syncthing or a Drive, and MemCP only reads and writes them with the care of SPACES.md. MemCP synchronises **memory** (facts, layout, guides, files it holds), not the disk.
- Not a shared memory between people: a person's memory stays that person's. Nodes belong to one person (or to a household that already shares spaces).

## Security of the swarm

A swarm holds the whole memory of a person, on several machines, so each of these is a rule and a test, not an intention.

**Identity and joining**
- *Today*: a node has a random id and joins by a person's answer or a one-time ticket (section above); the key pair, the signed membership and the keystore below are the target design, not what runs.
- Every node has a signing key pair (Ed25519), made on the node and kept in the system keystore, never in the data directory. A node is known by its public key.
- **Joining** is an act of the person, on a node they already trust: they ask it for a join code (shown as text and as a QR code, one use, ten minutes). The new node scans or types it and the two nodes exchange their public keys over an encrypted channel. Both then show the same **short check code** (a few words derived from both keys), and the person confirms on both screens that they match: this is what defeats a node in the middle. Until then the new node holds nothing.
- The list of nodes (the **membership**) is a document signed by the nodes that are administrators: who is in, with what role, since when. A node accepts a peer only if the membership says so, and a change of membership counts only if it is signed by an administrator. Roles: *administrator* (changes the membership), *member* (holds and syncs the spaces it is given), *blind* (stores encrypted spaces and cannot read them), *read-only* (receives, never sends).
- **Revocation** is a signed membership change: the others stop sending to the node and refuse what it sends; the spaces it held are re-keyed for new events (ENCRYPTION.md). A node that was offline when it was revoked learns it at its next contact and is refused.

**What a node accepts from another**
- Every event is **signed by the node that made it** and carries its space, its author and its logical time. A node checks the signature, that the author node may write that space, and that the event is well formed (size, tags, kind, dates) before it keeps it. An event that fails is not kept and is counted against the sender.
- **Per-peer limits**: a rate and a total size per hour and per space. A peer that exceeds them, or sends invalid events, is put in **quarantine**: its events are held aside, not merged, and the person is told on the page.
- **A clock that lies** cannot reorder history: a logical time that is too far ahead of the receiver's own is refused (a bounded drift), and the order of events is by hybrid logical time, never by wall clock.
- **Replay** is harmless: an event is identified by its id, so receiving it twice changes nothing.
- A peer's rights are those of its connection, as for any client: it cannot receive a space it was not given, and a sensitive space only if the person turned it on for that node.
- The **audit log** records, on every node, who joined, who was revoked, what was quarantined and what was refused, with the time, and the page shows it.

**In transit and at rest**: see TAILSCALE.md and ENCRYPTION.md. A join code never carries a secret key: it carries an address and a one-time code, and the keys are exchanged after.

## Managing the swarm

The page of any node (the administrator's) shows, for each node: its name, its role, what it holds (spaces and window), when it last synchronised, how far behind it is, and whether it is healthy, quarantined or revoked. From it the person can **join**, **revoke**, **change a role**, **choose which spaces each node holds and how** (send, receive, both, write only, window), **pause** a node, and **restore** a node from another.

- **A node gone for long** is flagged after a delay the person sets, and its administrator rights can be moved to another node without it.
- **Backups** are nodes: a node that is only ever sent to is a backup. Restoring means joining a new node and letting it receive everything.
- **Splits**: two groups of nodes that cannot reach each other keep working and merge when they meet. Nothing blocks writing in the meantime.

## Conflicts

Most differences are not conflicts: events are added, and the union of two histories is their sum. What remains are cases where two nodes changed the same thing without knowing of the other:

| Case | What is kept | What the person sees |
|---|---|---|
| The same note corrected on two nodes | Both corrections, each on the note it came from | The note shows **in dispute** with both versions side by side: *keep this one*, *keep the other*, or *write a third*. The choice is a new correction that replaces both. |
| A note retired on one node, corrected on another | The retirement and the correction | In dispute, the same way: *keep it retired* or *keep the correction*. |
| The same file under the same name with different content | Both files; the second is named with its node and time | Both listed, nothing overwritten |
| A tag or a keyword rule changed on two nodes | The newest change by logical time, per rule | The history of the rule shows both; the earlier one can be restored in one step |
| The layout of spaces (label, parent, position) | The newest change per field | Nothing, unless a move would break the tree (two nodes moving two spaces under each other): the later one is refused and reported |
| A dashboard document | The newest version | Both versions in its history |
| A guide (the person's instructions) | The version signed by an administrator node, newest first | If two administrators wrote different ones, both are shown and the person picks |

Rules that hold everywhere: **nothing is overwritten and nothing is erased by a merge**; a dispute never blocks reading or writing; an assistant sees a note in dispute with both versions flagged and never picks one; resolving a dispute is an act of the person and is itself an event in the history. A **dispute count** appears on the page and in the apps.

## Tests that decide when this is done

- Three nodes, one switched off, writes on all, every order of reconnection: the same memory on all three.
- A hostile node (bad signature, bad size, wrong space, clock in the future, replay) changes nothing but its own quarantine.
- Joining with a wrong check code fails; a revoked node is refused at its next contact; a join code works once and expires.
- Every conflict in the table above ends in a state the page can show and the person can resolve, and resolving it on one node is seen on the others.

## Order and cost

1. Event log with stable ids, hybrid logical clock and a "changed since" read on each space (most of it exists in the history already).
2. The node: a store, a key, a list of peers, an outbox, a catch-up exchange over one transport, tested with three nodes and one switched off.
3. Policy per space and per node, the page, adding and revoking a node.
4. Files by hash and chunks.
5. Disputes: showing and resolving them.

Honest cost: this is the largest piece of the plan, and the one where a mistake loses data. It is built last, with a test that switches nodes off and on in every order, and it must not delay the spaces, the mounts and the app.
