# Interface specification

**One app, the same everywhere**: iPhone, iPad, Mac, Windows, Linux and the node page in a browser show the same
four sections, in the same order, with the same words. A platform differs only where its device differs:

| Platform | What is its own, and nothing else |
|---|---|
| iPhone, iPad | a window of the memory (the last thirty days and what is pinned); Apple Health read and proposals written to it; the Watch and widgets |
| Mac, Windows, Linux | the whole memory; recording calls and meetings (everyone told, everyone agreeing) |
| Node page | the whole memory; managing the node (people, connections, peers, updates) |

Words never used: *bridge*, *capture program*, *capture app*, *proxy*, *vault* (say *space*, and *the app* or *the node*). Vulgar or slang words are never used in the interface.

## What matters most to a person (the order of everything below)

1. **Say it now, find it later.** A thought, a measure, a task, said in three seconds, filed by itself.
2. **What do I have to do, buy, remember?** The boards of each space answer, kept up to date from the notes.
3. **What happened, day by day?** One journal for everything, with each note's space.
4. **My health followed as a nurse would**: the health space's targets, their signs counted, a summary to hand to a doctor; on the iPhone, Apple Health in step (the one thing sold).
5. **Everything mine, on my machines**, reachable by the assistant I choose.

## Navigation

Four sections, a sidebar on a large screen (Mac, iPad, page), a tab bar on the iPhone:

```
┌─────────────────┬─────────────────────────────────────────────────────────────┐
│ ● Trace or drop │  the section selected                                       │
│ ● Journal       │                                                             │
│ ● Spaces        │                                                             │
│ ● Settings      │                                                             │
│─────────────────│                                                             │
│ ● in step       │                                                             │
└─────────────────┴─────────────────────────────────────────────────────────────┘
```

At the foot of the sidebar (or at the top of the iPhone's Note screen), one line: the state of the machines (in step,
syncing with its percent, offline, key refused with « Reconnect »). Proposals waiting (« 2 to confirm ») show as a
badge on Trace or drop.

### 1. Trace or drop

More than a note: a trace (said, written, measured) or anything dropped (a file, a photo, a document). The
image is unloading the mind: a thought drawn out of the head and put somewhere safe.


The microphone, large, in the middle; under it *Write*, *Attach a file*, and on a computer *Record a meeting*. A voice
note starts on a tap or on « Dictate » (widget, Action button, Siri, keyboard shortcut), **never on opening the app**.
Under the buttons, today's captures, newest first, each with the space it went to and its tags (`#home #shopping`),
or « To sort » while it waits. « To confirm » (proposals for Apple Health, Reminders, the calendar) opens from its badge.

### 2. Journal

Every note of every space the person reads, by day, newest first, each with its time, its space and its tags; a
structured trace (an app's day of measures) reads as values, never as JSON. Filter by space, tag or words. With
Pensine Health the days also carry the health journal (feelings, measures, Apple Health). « Context for an assistant »
makes a PDF of one space, with its dates.

### 3. Spaces

Every app has the same **Folders** / **Bento** switch in the Spaces toolbar and remembers the selection on that
device. Folders shows one tree with disclosure controls, a folder or space icon, the label and its note count.
Bento shows the tiles at their existing sizes and opens a folder onto its own tiles. Both order each level
alphabetically by the visible label in the interface language. The toolbar has **Organise**, **+**, and **Refresh**.
Organise lets people rename, move, add and drag to nest. A folder can be created empty.

A space opens on
**Targets**, its home: the targets with their signs, their figures and their progress; in Home and Hobbies, the
tasks to tick. Then **Journal** (one filterable chronology of notes and structured entries, with their time and
provenance), **Stats** and the **Inventory** of files. One **+** button, bottom right of every view, writes a note
or imports a photo or a file when the space is writable. The node page (Windows, Linux, Home Assistant) is the
desktop surface and reads the same contract (`memcp.view/1`).
A space also says **where it lives on this machine**: « Folder on this node », the path (by default the node's own storage
folder), a mode (notes and files both ways, or read only for search), the last pass and « Sync now ». A folder held by a
cloud service (iCloud Drive, Google Drive, OneDrive, Dropbox) is allowed and flagged with the node's advice to keep it on the
device. The setting is this machine's own and never goes to the others ([FOLDERS.md](FOLDERS.md)).

A person’s label is shown as entered. When a label was never set, the kind supplies a built-in label.
The node refuses new duplicates among the spaces one person owns, comparing case- and accent-insensitively and
ignoring surrounding spaces. It checks the default labels in both supported languages.

| Kind | Français | English |
|---|---|---|
| `inbox` | À trier | To sort |
| `health` | Santé | Health |
| `home` | Maison | Home |
| `hobbies` | Loisirs | Hobbies |
| `me` | Moi | Me |
| `work` | Travail | Work |
| `money` | Finances | Finances |
| `people` | Proches | People |

In the node page, **More** in a space holds its boards, notes, keywords and targets, tag tree, graph, assistant
connections, read-only connections and instructions. The keywords page also lists the **Sources**: the apps whose notes
always go to that space, before any keyword. By default, the notes of Chrome go to the Work space; a space that lists
a source turns that built-in rule off. The whole-memory graph is also reachable from Settings. The existing tile
shortcuts remain available in Bento.

### Health: a space, inside Spaces

Health is not a section of its own: it is the health space, in Spaces, with the boards every space has by default
(What's next, Ideas, Targets with their signs, lists) and its dashboard. Its sub-space *HealthApp* keeps the history of the
original health journal, apart from dictated notes, so one always knows where a measure comes from. A writable health
target lets the person add, remove and rename signs and symptoms and favouring factors. « Prepare an
appointment » (a PDF laid out as nurses hand over, SBAR) is free, on every app.

**What is sold**: the iPhone app's synchronisation with Apple Health (Pensine Health: 3 € a month or 9,90 € once). Nothing
else is paid. An ended subscription only pauses that synchronisation, and says so.

### 4. Settings

Machines (see below), Assistants
(connect one: address and key, or the line for Claude Code; rights per space), Engines (transcription: the device's
own or Whisper in the app; reading documents; summaries: Apple Intelligence when the device has it), Privacy (Face ID
or Touch ID at every opening, the veil in the app switcher), Pensine Health, Advanced (export, start over). On the node
page: People, Updates, the janitor.

**Machines** is the one place where nodes meet, the same on every platform and kept lean. In this order, and nothing else
above Advanced:

1. *This node*: its name, its **id** (`ABCD-EFGH-IJKL`), its addresses, and **Show my link**: the link and its QR code
   (`memcp://node?id=…&addr=…`), with a button to make a link without words (one use, ten minutes).
2. *The nodes*: one line each, with its state (in step, sending, receiving, paused, revoked), its last sync, its
   version, « older version » when it is behind, how it is reached and, when it told several addresses, **reachable via**.
   A node a peer told about (not yet linked) shows **Add this node**.
3. **Add a node**: one field (a link, an id or an address; the camera scans a code on a phone) and one button. This node
   asks THAT node; the three words appear here and on the other node; with a link made without words there are none.
4. *Pending requests*: a device or node asking to join shows on every node of the person, with its three words, the node
   that was asked and its id. Answering on one clears it everywhere.

Codes (show a code, type a code), the older way, are under Advanced, with the other less used controls. The routes are in
FEDERATION.md (« Adding a node »); none was removed.

## First opening

Two choices, on every app: **Start a memory** or **Join a memory**. When another device of the same Apple account is
already joined, a third comes first: « Continue with the memory of <device> » (an invitation in the iCloud Keychain,
one tap). Joining otherwise: the address, then « Ask for access » and the three words compared on the node's page.
Then, once, every permission the app will need, each with why. Then the use cases, in three lines.

## Golden paths (each one a test before a release)

**Everyone**
1. *An idea in the car*: « I have an idea for project Alpha at work » → filed in Work, a card in its Ideas.
2. *After a meeting*: the actions said are cards of What's next, each with the note it came from.
3. *« What do I need to buy? »*: Spaces › Home › Shopping, or Siri's « What do I need to buy in Pensine ».
4. *A second device*: install, « Continue with the memory of <device> », the same memory in a minute.
5. *An assistant*: Settings › Assistants › Connect, paste the line in Claude, ask « what's next at home? ».

**Health (the nurse's paths, from HealthApp)**
6. *A symptom noted*: « headache since this morning » → the sign counted under its target, the target's trend in Spaces › Health.
7. *A measure said*: « blood pressure 12/8 leaving the doctor's » → « Add to Apple Health? » (accept, always for this
   kind, always for Apple Health, refuse).
8. *A condition followed*: a target for migraine → its tool (attacks, triggers, treatment taken), its signs over
   ten days, red flags checked with the help numbers first.
9. *An appointment*: Spaces › Health › Prepare an appointment → the SBAR PDF of the period, shared with the professional.
10. *The subscription ends*: Health stays, Apple Health pauses, said in one line with how to resume.

**Exploring and managing the node page**
11. *Explore*: the graph of the whole memory, a tag clicked, the journal filtered, a note opened with its history.
12. *Organise*: a space created inside a folder, its keywords set, a note refiled by hand and the router learning it.
13. *Connect*: Claude Code on the same computer through `memcp mcp`; Home Assistant Assist through its MCP integration.
14. *Own it*: export everything with fingerprints; read it back on another node.
15. *Add a node*: Settings › Machines › paste or scan the other node's link, compare the three words (or none, with a
    link made without words on a node already held), the other node's person answers from any of their nodes; both become
    peers and the list shows the new node in step.

## What each screen reads

| Screen | Source |
|---|---|
| Machines map, sync | `GET /sync/status`, MCP `peers` (node, peers with version, last_seen, behind, exposures, only_local; connections with kind and via) |
| This node (id, link, QR), add a node | `GET /admin/api/node`, `POST /admin/api/nodes/add`, `POST /admin/api/nodes/invite`, `POST /admin/api/nodes/via`; the request: `GET /admin/api/join-memory?id=` |
| Pending requests | `GET /admin/api/join/pending` (also those of the peers), `POST /admin/api/join/approve`, `POST /admin/api/join/deny` |
| Space tree | MCP `spaces` |
| Dashboard, Targets, timestamped register | MCP `space_view` (`memcp.view/1`, [CONTRACT.md](CONTRACT.md)); `GET /admin/api/space/view`; `living_set` for a card move |
| Boards and lists | MCP `living_get`, `living_set` |
| Journal | `GET /admin/api/journal`; on a device, the local copy (`node_recent`) and `living_get` on `index` |
| Graph | `GET /admin/api/links[?space=]` |
| Tags, keywords | MCP `stats`, `rules_get`, `rules_set` |
| Proposals | the new notes of the copy, read on the device (`note_proposals`) |

## Acceptance

1. The same four sections, in the same order, on every platform; a function missing on one is a defect.
2. No screen shows a technical name, JSON, a forbidden word or a vulgar word.
3. The golden paths above pass on the iPhone, the Mac and the page.
4. Every text exists in French, English and Spanish.
