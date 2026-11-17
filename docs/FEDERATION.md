# Peers: every node is a copy, and together they are the backup

Status: plan. Built after the spaces (see SPACES.md), the mounted folders and the app.

## The idea

There is no central server. A MemCP **node** is a program with a local store and a list of peers: a server, a bridge on a laptop, a Windows PC, a phone. Nodes synchronise with each other the way torrent clients and Syncthing do: any node that has something gives it to any node that lacks it. A node that holds everything is a complete copy, so **synchronisation and backup are the same thing**: losing a machine loses nothing as long as one other node holds the memory.

A node can be switched off for days. When it comes back it asks its peers what it missed and sends what they missed. Nothing needs to be online at the same time and nothing needs a hub.

One node may be the **admin**: the one whose page the person uses to see all the nodes, to add or revoke one, and to set what each may hold. In a home installation that is the Home Assistant app. It is a role, not a different kind of program: any node can take it over.

## How a person grows into it

1. **One Windows PC.** The person installs the app (Kalios). All the data is on this PC, in a folder an assistant such as Claude can read, with the instructions in it. No server, nothing else.
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

## What this is not

- Not a replacement for a file synchroniser: mounted folders can still be synchronised by Syncthing or a Drive, and MemCP only reads and writes them with the care of SPACES.md. MemCP synchronises **memory** (facts, layout, guides, files it holds), not the disk.
- Not a shared memory between people: a person's memory stays that person's. Nodes belong to one person (or to a household that already shares spaces).

## Security of the swarm

A swarm holds the whole memory of a person, on several machines, so each of these is a rule and a test, not an intention.

**Identity and joining**
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
