# Changelog

## 20261116.0.3

- `triage_claim` and `triage_release`: a sorter outside the memory (an assistant on a device) reserves notes of À trier for a few minutes, so that two sorters never take the same.
- The pairing text no longer says everything lands in À trier.
- `/mcp/<person>/<connection>` is accepted beside `/mcp/<connection>`: the address says whose connection it is.
- On the page, joining a network comes first and connecting an assistant second.

## 20261116.0.2

- `memcp init --defaults`: the usual spaces (personal, health, home, finances, hobbies, work) in one go.
- An assistant that connects is told when notes wait in À trier, and asked to tell the person and offer to sort them.
- The janitor's own connection no longer shows among the connected assistants; `spaces` counts 0 for an empty folder.
- Planned in docs/MERGE.md: merging two memories.

## 20261116.0.1

Versions are now the day of the release, then a counter: `AAAAMMJJ.0.N`, for every app.

- The `spaces` MCP tool: the tree of the spaces a connection can read, with label, parent, kind, rights and counts.
- The add-on has an icon and a logo.

## 0.25.0

- **The spaces as folders** on the page: a tree you open and close, with drag and drop to nest or reorder, a `+` to make a space inside another, rename in place. Opening one shows its **tags** and its **keywords**; drag a tag onto another space to send the notes it carries there (previewed first, never erased: each note is retired in the source with the reason "moved to …"). The tags that file notes in a space are kept in its settings, with a history.
- **Joining by request**: a device that finds a node asks to join, the person compares **three words** on both screens and allows it; the key comes once. The join window on the page says **Connected** with the name as soon as a device joins, and waiting requests show up as a banner and a browser notification.
- **Discovery**: a node advertises itself on the local network (`--advertise`, mDNS `_memcp._tcp`) and answers `/.well-known/memcp`; `memcp-bridge discover` finds nodes on the network and on the tailnet, and a Home Assistant installation by its own names; `memcp-bridge join` asks one to let this computer in.
- Moving notes between spaces by tag in the core (`move_by_tag`).
- A shorter, plainer README; the full manual is in `docs/REFERENCE.md`.

## 0.24.3

- The Home Assistant app takes in the data left by another installation (a former HemCP app) in its own configuration folder, once, keeping every key and address (see DOCS.md, *Moving from another app*).

## 0.24.2

- **Windows and macOS are first-class**: the whole test suite runs and passes on Linux, macOS and Windows in CI, and the release carries the programs of the three systems.
- **Fix (Windows):** an export listed its files with backslashes while its manifest and its archive use forward slashes, so a fresh export failed its own check and the page's "export my data" was refused; resolved paths lost the `\\?\` prefix that SQLite's `VACUUM INTO` takes badly.
- A flaky bridge test waits for its port to close.

## 0.24.1

- **Fix:** `memcp-bridge transcribe` refused to start with a speech service and no local model (`transcribe needs whisper_model`). It now needs a local model or `whisper_api_url`, so the Home Assistant app's voice pass works. Checked end to end: a voice note recorded on the page, read through a speech service, becomes a dictation note in the inbox.
- `memcp-bridge connect ADDRESS` carries an assistant's standard input and output to the bridge's HTTP gateway on this machine (the way on Windows).
- The test suites run on macOS and Windows in CI; the Unix-only tests are gated.

## 0.24.0

First release under the AGPL-3.0-or-later (with a commercial licence available); earlier versions stay Apache-2.0.

- **Voice notes and attachments on the page of a node**, the same functions as on the computers: **Record** keeps a voice note from the browser (WebM, or m4a on Safari), **Attach files** keeps photos, PDF, text and recordings, each as a note of the space with its file, which can be opened or played.
- **Voice notes read by a speech service**: the bridge can send a recording to a service of the OpenAI kind (`whisper_api_url`, `whisper_api_model`, `whisper_api_key`) instead of a local speech model, and the Home Assistant app can run that pass (`transcribe` options), so that a voice note recorded on the page becomes text there too.
- The desktop bridge (`memcp-bridge`, `memcp-bridge-ffi`) joins this repository, and the add-on image carries it.
- The Home Assistant app takes the page's new explorer: dashboards, tree, notes and their history, and the layout of spaces.
- `counts` sections can count the terms cited in the notes and the notes under each child of a tag, in any space.
- Recordings made by a browser (WebM with Opus, fragmented MP4) are accepted as audio.


- The desktop bridge (`memcp-bridge`, and its Swift interface `memcp-bridge-ffi`) joins this repository: one repository for the server and the desktop bridges.
- `counts` sections can count the terms cited in the notes and the notes under each child of a tag, in any space.

## 0.23.1

- The page opens each space: a dashboard (notes day by day; for a health space, the signs and symptoms counted from the journal and the targets), the tree of tags, and the notes, which can be searched, added, corrected and removed with their history kept.
- The layout of spaces on the page: rename, nest, reorder, add a space of one's own.
- The page says *space* where it said *vault*.
- The add-on image keeps `curl` (the options of the app are read with it).

## 0.23.0

First public release, under the name MemCP (the project was developed privately as HemCP up to 0.22.7).

- Spaces can be laid out: a label, a parent and a position, nesting up to eight deep, free creation and renaming, without touching a file or a right.
- Dependency fix: SQLite updated through `rusqlite` 0.41 (RUSTSEC-2026-0459). The minimum Rust version is now 1.89.
- Designs for mounted folders, locking with a passphrase or a key, a swarm of peer nodes, and one configuration screen on every platform (see `docs/`).
- A first security review (`docs/reviews/`).

An installation from before the rename keeps working: the data is read as it is, and the Home Assistant add-on keeps its slug when it is updated in place (see `docs/MIGRATING.md`).
