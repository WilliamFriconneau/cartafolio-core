# Spaces: organising, nesting, mounting folders, backing up

Status: plan. Phase 1 is built first; the others follow in order.

A **space** is the unit of rights (one SQLite file). A person organises their spaces freely: renames them, nests them, reorders them, merges them. This document says how, without breaking the rule that a right is granted on a space only.

## 1. A tree of spaces (phase 1)

- The **name** of a space (its slug, its file name, what a connection's rights refer to) never changes. Everything a person organises is **layout**, stored beside it in the control database: a `label` (free text, any language, 1 to 64 characters), a `parent` (another space of the same person, or none) and a `position` (the order among siblings).
- Renaming, moving and reordering touch the layout only: no file is renamed, no right is rewritten, nothing is lost. That is what makes them safe to offer at any time.
- A person creates a space of their own with a label; the slug is derived from it (`<person>-<label>`, shortened, made unique). Its kind is `custom`.
- Rules: only the owner of a space lays it out; a parent must belong to the same owner; no cycle; a depth of at most 8.
- **Rights do not follow the tree.** A connection with the right on `Perso` does not read `Perso/Health` unless it holds the right on that space too. The tree is for people; the boundary is the space. A tool that lists spaces returns the tree of the spaces a connection can reach, and nothing about the others, not even their names.
- A sensitive space (health, money) is never reached through a parent: reading a parent never returns facts of a child the connection has no right on (the existing rule, tested again for children).
- Tags keep organising what is *inside* a space (`work/dev`). A sub-space is for a boundary of rights; a tag path is for a topic.

### Merge and retire (phase 2)

- **Merge** space A into B: every fact moves, history included, the rights of A are granted on B, A is retired. Done as one transaction per space file pair, verified fact by fact before A is retired (the way the converter was).
- **Retire** a space: hidden from the tree, kept on disk, reversible. Nothing is erased.

## 2. Folder-backed spaces (phase 3)

A space can be fed by **several folders scattered on the machine** (Markdown, text), as a drive does.

- A *mount* is a **logical folder** attached to a space (or a sub-space): it has an id and a name, and each node that holds it says **where it is on its own disk** (or that it does not hold it). The same folder is `~/Drive/Work` on one computer and `D:\\Work` on another, and a bridge picks, on its own machine, the path and the space it feeds. The mount has a mode: **source** (the folder is the truth: changes made by hand are picked up, the facts follow) or **export** (the memory is the truth: the folder is rewritten from it). Both modes exist. A space may have several mounts of either mode.
- Picking up changes: a scan at start and at an interval, then file notifications where the system gives them. A file is one passage, tagged by its relative path. A deleted file retires its passage; nothing is erased.
- The first uses: an Obsidian vault, a `Code` folder of notes and specs, a Drive folder synchronised to disk.
- Safety: a mount is added only by the person, from the page; a path is resolved and refused if it leaves the mounted root (links included); files above a size are skipped and listed.

### One synchroniser per folder

When the same folder is held by several nodes (a laptop's bridge and the Home Assistant hub), the files are replicated between them the way Syncthing does it: by content hash, in chunks, a file only when it changed, a conflict kept as a copy. MemCP does not become a second drive:

- A folder already synchronised by a drive (Google Drive, OneDrive, iCloud) is marked *managed elsewhere* on that node: MemCP only reads it there. Two synchronisers writing the same files make echoes and conflicts.
- Replication is per mount and per pair of nodes, chosen by the person: *files and passages*, *passages only* (the files stay where they are), or *nothing*.
- The node that holds the drive copy sends its files; the other node keeps its own copy outside any drive and is the one that can be restored from a backup.

### Living next to a synchronised drive

MemCP is also a manager of folders that other programs synchronise (Google Drive, Syncthing, iCloud, OneDrive). Those programs move, duplicate and half-write files at any time, so the rules are strict:

- **The memory's own data never lives in a synchronised folder.** The SQLite files, the keys and the control database stay on a local disk of the machine that runs the server. A database copied by a sync tool while it is written is a corrupted database. Only the mounted folders and the backup archives may be in a synchronised folder. The server refuses to start with its data directory inside a mount, and says why.
- **Settle before reading.** A file is read only once its size and modification time have not changed for a few seconds, so a file being written or uploaded is not ingested half-done.
- **Online-only files are not deletions.** A drive may show a file without holding its content (a placeholder). Such a file is listed as *not available* and is neither read nor treated as gone; a read has a time limit.
- **A vanished folder is not a mass deletion.** A passage is retired only when its file has been missing across several scans. If a whole mount becomes unreadable, or a large share of its files disappears at once (a drive unmounted, a sync restarting), the mount is paused and the person is told. Nothing is retired in that case.
- **Conflict copies are recognised, never merged silently.** `name (1).md` (Google Drive), `name.sync-conflict-…` (Syncthing), `name-<machine>.md` (OneDrive) are kept as passages of their own, tagged as a conflict, and shown to the person next to the original.
- **Write rarely, write atomically.** In *export* mode a file is rewritten only when its content really changes (compared by hash), through a temporary file of the same folder then a rename, so that a sync tool never sees a half file and nothing is touched for nothing (no storm of modification times). The hash of what the memory last wrote is kept: if the file on disk differs from it, someone edited it by hand, and the memory writes a conflict copy beside it instead of overwriting.
- **One writer per file.** In *source* mode the memory writes nothing in the folder. In *export* mode it is the only writer of the files it makes; a hand edit is detected as above.
- **What not to scan.** `.git`, `node_modules`, `target`, `.DS_Store`, temporary and hidden files, and anything in the mount's ignore list. A code folder is mounted with these ignores by default.
- **Windows and macOS names.** Names are compared without case where the system does not tell them apart, paths are normalised, and reserved or too long names are skipped and listed rather than failing the scan.

## 3. Running on Windows (phase 4)

- **State.** The whole workspace (the server, the bridge, the janitor and the Swift interface) builds on Windows in CI, and the releases carry a Windows build of `memcp`, `memcp-capture` and `memcp-janitor` next to the macOS and Linux ones. What is not done: the **test suites** do not run on Windows yet (several use Unix file modes), and the bridge's gateway over a Unix socket is Unix only: on Windows, run the gateway with `memcp-capture --listen 127.0.0.1:PORT` and give the assistant `memcp-capture connect 127.0.0.1:PORT`, which carries its standard input and output to it (this form works on every system, and only on the loopback).
- **Private files.** Unix keeps a data folder at `0700` and its files at `0600`. Windows has no such bits: a folder takes the access list of the folder it is made in, so the data directory belongs **under the person's own profile** (for example `%LOCALAPPDATA%\MemCP`), never in a shared folder. The code treats these calls as no-ops there (`fsx`), and `memcp doctor` does not check modes on Windows.
- **Running.** `memcp serve` as a plain program, or as a service of the person's own account. The folders it mounts are any local or synchronised folder; paths with spaces, drive letters and long names are the next thing to test.

## 4. Backup with the bridges (phase 5)

- The memory already exports a consistent snapshot. A **bridge** (a small local client) asks for it on a schedule and writes it to a folder the person chose: another disk, a Drive folder, a NAS.
- A snapshot is one archive of the spaces the person owns, with a manifest of hashes, so that a restore can be checked. It carries the layout (the tree), the guides and the history.
- A bridge never needs more than the right to read for backup; restoring is a deliberate act on the server by the person.

## Order

1. Layout: label, parent, position; create, rename, move, reorder. Core, then page and tools, with tests.
2. Merge and retire.
3. Folder-backed spaces.
4. Windows.
5. Backup with the bridges.
