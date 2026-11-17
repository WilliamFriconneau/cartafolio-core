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
┌──────────────┬─────────────────────────────────────────────────────────────┐
│ ● Trace/drop │  the section selected                                       │
│ ● Journal    │                                                             │
│ ● Spaces     │                                                             │
│ ● Settings   │                                                             │
│──────────────│                                                             │
│ ● in step    │                                                             │
└──────────────┴─────────────────────────────────────────────────────────────┘
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
or « Inbox » while it waits. « To confirm » (proposals for Apple Health, Reminders, the calendar) opens from its badge.

### 2. Journal

Every note of every space the person reads, by day, newest first, each with its time, its space and its tags; a
structured trace (an app's day of measures) reads as values, never as JSON. Filter by space, tag or words. With
MemCP Health the days also carry the health journal (feelings, measures, Apple Health). « Context for an assistant »
makes a PDF of one space, with its dates.

### 3. Spaces

The tree of spaces (folders open in place, drag and drop, labels never technical names). A space opens on **its
boards**: What's next, Ideas, Targets (with their signs), and its lists (shopping…), each card ticked or moved in one
tap, each board saying its last pass and how many notes came since. Then its notes, its tags, its keywords (the
words that file a note there), and its **graph**: spaces, tags and keywords linked, a click filters the notes.

### Health: a space, inside Spaces

Health is not a section of its own: it is the health space, in Spaces, with the boards every space has by default
(What's next, Ideas, Targets with their signs, lists) and its dashboard. Its sub-space *HealthApp* keeps the history of the
original health journal, apart from dictated notes, so one always knows where a measure comes from. « Prepare an
appointment » (a PDF laid out as nurses hand over, SBAR) is free, on every app.

**What is sold**: the iPhone app's synchronisation with Apple Health (MemCP Health: 3 € a month or 9,90 € once). Nothing
else is paid. An ended subscription only pauses that synchronisation, and says so.

### 4. Settings

Machines (the map: assistants in the cloud and through which node, assistants on the machines, every node with its
version, last activity and how it is reached; a machine reachable only locally « syncs on opening »), Assistants
(connect one: address and key, or the line for Claude Code; rights per space), Engines (transcription: the device's
own or Whisper in the app; reading documents; summaries: Apple Intelligence when the device has it), Privacy (Face ID
or Touch ID at every opening, the veil in the app switcher), MemCP Health, Advanced (export, start over). On the node
page: People, Updates, the janitor.

## First opening

Two choices, on every app: **Start a memory** or **Join a memory**. When another device of the same Apple account is
already joined, a third comes first: « Continue with the memory of <device> » (an invitation in the iCloud Keychain,
one tap). Joining otherwise: the address, then « Ask for access » and the three words compared on the node's page.
Then, once, every permission the app will need, each with why. Then the use cases, in three lines.

## Golden paths (each one a test before a release)

**Everyone**
1. *An idea in the car*: « I have an idea for project Alpha at work » → filed in Work, a card in its Ideas.
2. *After a meeting*: the actions said are cards of What's next, each with the note it came from.
3. *« What do I need to buy? »*: Spaces › Home › Shopping, or Siri's « What do I need to buy in MemCP ».
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

**The tinkerer (the node page, Obsidian-like)**
11. *Explore*: the graph of the whole memory, a tag clicked, the journal filtered, a note opened with its history.
12. *Organise*: a space created inside a folder, its keywords set, a note refiled by hand and the router learning it.
13. *Connect*: Claude Code on the same computer through `memcp mcp`; Home Assistant Assist through its MCP integration.
14. *Own it*: export everything with fingerprints; read it back on another node.

## What each screen reads

| Screen | Source |
|---|---|
| Machines map, sync | `GET /sync/status`, MCP `peers` (node, peers with version, last_seen, behind, exposures, only_local; connections with kind and via) |
| Space tree | MCP `spaces` |
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
