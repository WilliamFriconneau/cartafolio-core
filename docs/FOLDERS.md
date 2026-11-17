# A folder for each space, on each node

Every space can point, **on each node**, to a local folder of the person's choice. The node keeps the space's notes and files there as Markdown and readable files, and keeps them in step **in both directions**: an edit made in the folder comes back into the memory as a new version of the note; a new `.md` or a new file in the folder becomes a note or a file of the space.

This is one setting. It replaces the read-only « Brancher un dossier » (the *library*) and the Markdown connectors of the apps: `library add` is now an alias that sets a read-only folder.

## The rules, in short

- **Per node.** The folder is the node's own business. It is stored in the node's control database (`space_folders`, `folder_state`), never in a space file, and it is in no sync event: it does not travel in the swarm. `/Users/me/Notes` means nothing on a phone, and the Home Assistant hub has its own folder, or none. A test reads the whole sync log after a pass and finds no path in it.
- **By default, no folder is mirrored.** A space with no folder uses the node's own storage folder (`<data>/files/<space>`: the bytes of its files, named by their digest). `folder_get` says it (`custom: false`, `mode: null`, `path` = `default_path`).
- **The memory stays the memory.** Nothing is ever erased. A folder edit is a `supersede` (the old version stays in the history), a deleted file is a `retire` with a reason, a conflict keeps both sides.
- **Settled files only.** A file is taken once its last change is three seconds old, so a copy in progress is never read half-done. Hidden files and folders (`.obsidian`, `.git`), links, and names that say a file is not finished (`.part`, `.tmp`, `.crdownload`, `~`) are left alone.

## What is in the folder

```
<folder>/
  boiler-serviced-in-march-4gk2xq.md     a note: <slug>-<shortid>.md
  roof-estimate-9ab3cd.md
  files/
    plan.pdf                             a file of the space, as itself
    photo-fuse-box.jpg
```

A note is `<slug>-<shortid>.md`: the slug is the first words of the text (lowercase ASCII, at most 48 characters), the short id the first six characters of the note's id. The name never changes once written, even when the note gets new versions. A name already taken by a file the node did not make is never overwritten: the node picks another one.

### Front matter

```markdown
---
id: f_4gk2xq7m1pab
space: home
tags: [home/boiler, home/heating]
created: 2026-11-17T09:12:00Z
version: 2
---

Boiler serviced in March.
```

| Key | Meaning | Read back from the folder? |
|---|---|---|
| `id` | The id of the version the node wrote. Used to recognise a renamed file. | Only to follow a rename; it is not trusted for anything else, and an old id changes nothing. |
| `space` | The space. | No. |
| `tags` | The tags of the note, as `[a/b, c]`, `a, b`, or a list of `- a` lines. | **Yes.** Changing it makes a new version of the note with these tags. Without a `tags` key, the note keeps its tags. |
| `created` | When the first version of the note was written (ISO 8601). | No. |
| `version` | How many versions the note has (1 for a new note). | No. |

The text is what follows the front matter. A note's text is stored with its white space folded (single spaces): changing only the line breaks or the spacing of a file is **not** a new version. A file with no front matter is all text.

## Both directions

Every pass (see *When it runs*) does, in this order:

1. **The folder to the memory.**
   - A tracked note file changed (by size and time, then by hash) → `supersede` of the note with the new text and tags, author `folder-sync`, reason `edited in the folder on this node`.
   - A tracked file of `files/` changed → the new content is put as a file of the space (`file_put`) and the old record is retired (`replaced by an edit in the folder`).
   - A file the node did not know → a new note (a `.md` or `.markdown` outside `files/`) or a new file of the space (anything else, anywhere), author `folder-sync`. A `.md` whose text is already an active note of the space is *adopted* (it becomes that note's file) instead of being duplicated, which is how a folder made by an earlier setting is picked up again.
   - A tracked file that is gone → see *Deleting*.
   - A file that is renamed or moved (same content, or same `id` in its front matter) keeps its note.
2. **The memory to the folder.** Every active note and file of the space the folder does not show yet is written (atomically: a temporary file in the same folder, then a rename, so a sync tool never sees half a file). A note that got a new version elsewhere is rewritten in its own file. Notes the node keeps for itself (the filing rules, the boards, the descriptions of files) are not mirrored.
3. **The index.** The space's library index is refreshed over the same folder, so `library_search` finds what is in it (including `.docx` and `.pdf` by name).

### Loop protection

The node remembers, for each file it wrote, the hash of what it wrote and the size and time it then had. A file whose size and time are unchanged is not even read; a file touched but with the same hash (a sync tool, a copy back) is not a change. What the node wrote is never read back as an edit. The note's front matter is only rewritten when the **memory** changes; after an edit made in the folder, the file is left exactly as the person wrote it.

### A conflict keeps both

If a note changed on **both** sides since the last pass (an edit in the folder and a new version in the memory, here or from another node), the memory's version stays the note; the folder's copy becomes **a new note** tagged `conflict` (and its own tags), and the file is rewritten with the memory's version. Nothing is lost, and the person sorts it out from the tag. The conflict note then gets its own file.

### Deleting

- **A file deleted from the folder retires its note** (or file) with the reason `its file was deleted from the folder on this node`, author `folder-sync`. The text stays readable in the history.
- **The reverse never happens silently.** A note retired in the memory leaves its file in the folder. The node keeps that file as *retired* and does not write it again; if the person edits it later, the edit becomes a new note.
- A note that changed in the memory since the last pass is not retired by a deletion: its file comes back.
- **A vanished folder is not a mass deletion.** If the folder itself is not there (a drive unmounted), the pass stops with an error and changes nothing. If five or more tracked files are gone at once and they are more than half of the tracked files, nothing is retired and the pass says why.

### What goes wrong is said, not repeated

A file the node cannot take (an empty note, a bad tag, a file type the memory does not keep, a note that duplicates another) is set aside with its reason and looked at again only when the file changes. The reasons are in `errors` of `folder_get` and on the page.

## Modes

| Mode | The folder | The memory |
|---|---|---|
| `readwrite` (default when a folder is chosen) | the notes and files are written there | edits, new files and deletions come back |
| `read` | only read, for search: **nothing is written in it** | **nothing comes into it**: no note is made from the folder |

`read` is what `library add` sets: an existing vault (Obsidian, a folder of documents) is indexed for `library_search` and never copied, changed or moved. In `read` mode a pass only refreshes the index.

## Cloud folders

A folder held by a cloud service is allowed, read and write. The node recognises it from the **path** (the names of the folders on the way, case ignored) and flags it:

| `cloud` | Path patterns |
|---|---|
| `google-drive` | `GoogleDrive-*` (macOS `Library/CloudStorage`), `Google Drive`, `My Drive` |
| `icloud` | `Mobile Documents`, `com~apple~CloudDocs`, `iCloud Drive` |
| `onedrive` | `OneDrive`, `OneDrive - <organisation>`, `OneDrive-<name>` |
| `dropbox` | `Dropbox`, `Dropbox (<team>)` |

`folder_get` then returns `cloud` and a `warning` text: keep the folder **on the device** (always available offline). A file the cloud has not downloaded cannot be read, and two programs that write the same files make copies and conflicts. The node reads such a file only when it can, and says so when it cannot. The app shows the advice next to the folder.

## Limits and refusals

- The path is the full path on the node's machine. The folder is made if it does not exist.
- In `readwrite` mode the folder must be **outside the node's data directory** and must not hold it (the databases must never sit in a folder that others write), and must not overlap the folder of another space on this node. The root of the disk is refused.
- A file over 24 MB is left alone (the biggest a space keeps, so that it can reach every node). Folders are read 12 levels deep.
- One pass at a time on a node, across processes (a lock file in the data directory): `memcp folder sync` while the node serves waits its turn or says a pass is already running.
- The person chooses the folder: through the page (administrators), the command line, or an assistant with write rights on the whole space. The assistant's tool, `folder_set`, names a path on the machine; give that right as you would any write right on the space.

## When it runs

While the node serves (and in the apps' embedded node), a background task makes a pass over every space that has a folder, every **10 seconds** (`MEMCP_FOLDER_EVERY`, at least 1). A node with no folder set does one cheap query per pass. A pass can be asked for at any time: `memcp folder sync [SPACE]`, the **Sync now** button of the page, `POST /space/folder {op: "sync"}`. Setting a folder from the command line also runs a first pass.

## Where to set it

| Where | How |
|---|---|
| MCP tools | `folder_set {space, path \| null, mode?}` (write on the whole space), `folder_get {space?}` (read). See [TOOLS.md](TOOLS.md). |
| Command line | `memcp folder set SPACE PATH [--read]`, `memcp folder set SPACE --default`, `memcp folder get [SPACE]`, `memcp folder sync [SPACE]`; `memcp library add PATH --space NAME` is `folder set NAME PATH --read`. |
| HTTP | `GET /admin/api/space/folder?space=`, `POST /admin/api/space/folder` (see [CHANGELOG-API.md](CHANGELOG-API.md)). |
| Node page | Spaces, a space selected, side panel: **Folder on this node**: the path, the mode, the cloud advice, the last pass, *Sync now*, *Back to the default folder*. |

`folder_get` returns `{space, path, default_path, custom, mode, cloud, warning, last_scan, files, errors}`: `path` is the folder in use, `default_path` the node's own storage folder, `last_scan` the end of the last pass (ISO 8601), `files` the notes and files tracked, `errors` what the last pass could not take. Setting `path` to `null` (or to `default_path`) stops the mirror and **leaves the folder as it is**.

## In the code

`memcp_core::folders`: `set`, `get`, `all`, `info`, `sync_space`, `sync_all`, `cloud_of`, `cloud_warning`, `slug`, `short_id`, `hash_bytes`, `parse_note`, `render_note`, `Mode`, `Options`, `Report`. The background task is `memcp_server::folder_sync`. The tests are `crates/core/tests/folders.rs` (unit, round trip, conflict, deletion, read-only, loop, cloud, permissions, nothing in the sync log), `crates/server/tests/folder.rs` and `crud.rs` (command line, task, routes) and `crates/server/tests/page/folder.test.js` (the page).

See also [SPACES.md](SPACES.md) (living next to a synchronised drive) and [INTAKE.md](INTAKE.md) (a watched folder is a drop box; a folder of a space is kept in step).
