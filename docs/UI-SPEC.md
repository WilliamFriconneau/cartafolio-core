# Interface specifications

One memory, many machines. Every screen below shows the same memory, in the same words. Words that are
not used anywhere: *bridge*, *proxy*, *vault* (say *space*). A machine that joins is a **peer** (a node).

## Principles

1. **Machines first.** The first thing on screen is which machines hold this memory, and whether they are in step.
2. **One memory.** After joining, a device shows one memory. A local copy is a replica, never a second memory.
3. **Live.** A synchronisation in progress is visible as it happens: which machine, which direction, how far.
4. **Two choices at the start**, on every app: *Start a memory* or *Join a memory*.
5. **Order of actions**: join a machine first, connect an assistant second.
6. **Each machine lists its own connections** (assistants and peers), with last activity.
7. **Parity.** Every function and option exists on the node page, the desktop apps and, where the screen allows it,
   the phone. What a small screen cannot show it says so and offers the page.
8. Folders: a space that has children is a **folder** (no count, no "connections reach it"); only leaf spaces are
   opened. Labels, never technical names.

## Layout (node page, Mac, Windows, Linux)

```
┌───────────────┬──────────────────────────────────────────────┐
│ MACHINES      │  <machine or space selected>                  │
│ ● This computer│                                              │
│   ◐ Server  ↓ 63 %│  Tabs: Journal · Dashboard · Notes · Tree │
│   ● iPhone     │                                              │
│ + Join a memory│  Right column: connections of this machine   │
│ ─────────────  │  (assistants, peers) with status dot and     │
│ SPACES (tree)  │  last activity; "Connect an assistant" here  │
│ ▸ Perso        │                                              │
│ ▸ Home         │                                              │
│ Settings       │                                              │
└───────────────┴──────────────────────────────────────────────┘
```

- Status dot per machine: green in step, amber catching up (arrow shows direction, percent shown), grey offline,
  red refused (key cancelled: a button to join again).
- The footer of the window always shows: memory name, machine count, last sync time.

## Node page (served by every node)

- Sidebar: machines (this one first), then the space tree (folders open/close, drag and drop, "+" on a folder).
- Machine view: name, address, version, peers seen, connections it holds, live sync (see below), a button to
  forget the machine.
- Sync block, one line per peer: `name  ↑ sending | ↓ receiving | = in step   42 %   1 380 / 3 290 notes   2 s ago`,
  with a progress bar; refreshed every 2 s; `GET /sync/status` is its only source. The block lists every peer of the
  memory, not only the ones that joined this node: a peer known through another node has no bar, and shows
  `known through <node>`, its version, `older version` when it is behind, how it can be reached (`loopback`, `lan`,
  `tailnet`, `public`) and, when it can only be reached locally, `syncs when opened with the peers it can reach`.
  A peer not seen for 90 days is dimmed (`gone`). A revoked peer is not listed.
- Join banner: when a machine asks to join, a banner shows its name and the three words; one button to allow, one
  to refuse. The same request appears as a notification on the desktop apps.
- Space view: Journal (by day), Dashboard (sections from the `dashboard` note), Notes (tags and keywords of the
  level, filter), Tree (tags). Record a voice note, attach a file, write a note. A correction keeps the old text
  in history.
- Settings: language, public address, assistants and their rights (per space, read or write), instructions,
  the janitor (keywords it keeps, the model it may use, never the default), export, merge two memories.

## Desktop apps (Mac, Windows, Linux)

- Same layout as the node page, native. Menu-bar icon: state (in step / syncing with percent / offline), quick
  note, record.
- Start screen when no memory: **Start a memory** / **Join a memory** (address, QR, or "ask to join" with the
  three words). Nothing else.
- The main window can be the microphone alone (record, a number under it for the notes taken today).
- Preferences in the sidebar: Memory, Where captures go, Recording, Spaces, Connections, Activity, General.
  "Where captures go" never offers a proxy for an assistant when the memory is on this computer.
- A local node on a desktop is a **replica** of the joined memory: copy complete, kept in step, shown as one memory.

## iPhone and iPad

- Home: the memory (journal of the day, quick record, quick note). Tabs: Memory, Machines, Spaces, Settings.
- Machines tab = the sidebar above as a list: each machine with dot, direction and percent; tap for its
  connections.
- Joining: scan the QR of a node, or type the address; "ask to join" shows the three words and waits.
- iPad uses the desktop layout (sidebar + content + connections column).
- Health (Apple Health) is the one paid option, as a module; everything above is free.

## Watch and widgets

- Watch: record, state dot (in step or not), today's count. Complication: the state dot and the count.
- Widgets: quick record; last note; sync state.

## What each screen reads from the node

| Screen | Source |
|---|---|
| Machines, live sync | `GET /sync/status` (`node`, `peers`: name, version, last_seen, learned_from, behind, gone, exposures, only_local, and for direct peers direction, spaces, done, total, percent, last_activity; `connections`: name, kind cloud or local, via, last_used_at) |
| Space tree | MCP `spaces` (name, label, parent, position, kind, writable, facts) |
| Tags | MCP `stats` (tags with counts) |
| Graph | MCP `links` (planned) |
| Library | MCP `library_search`, `library_raise` (planned) |
| Waiting notes | `triage_claim`, `triage_release` |
| Keywords of a space | the `rules` note (tag `rules`); MCP `rules_get` / `rules_set` (planned) |

## Acceptance (what is checked before an app is called done)

1. Start a memory on a machine, join it from a second one, write on one, see it on the other.
2. During a copy, the percentage rises and the direction is right on both machines.
3. A machine that is switched off shows grey, and catches up when it comes back.
4. No screen contains the words listed at the top.
5. The same list of functions and options is reachable on the node page and on each desktop app.
