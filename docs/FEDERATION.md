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

## What this is not

- Not a replacement for a file synchroniser: mounted folders can still be synchronised by Syncthing or a Drive, and MemCP only reads and writes them with the care of SPACES.md. MemCP synchronises **memory** (facts, layout, guides, files it holds), not the disk.
- Not a shared memory between people: a person's memory stays that person's. Nodes belong to one person (or to a household that already shares spaces).

## Order and cost

1. Event log with stable ids, hybrid logical clock and a "changed since" read on each space (most of it exists in the history already).
2. The node: a store, a key, a list of peers, an outbox, a catch-up exchange over one transport, tested with three nodes and one switched off.
3. Policy per space and per node, the page, adding and revoking a node.
4. Files by hash and chunks.
5. Disputes: showing and resolving them.

Honest cost: this is the largest piece of the plan, and the one where a mistake loses data. It is built last, with a test that switches nodes off and on in every order, and it must not delay the spaces, the mounts and the app.
