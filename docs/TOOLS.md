# Tools and connections, rethought

Status: design, and the plan of the refactor. The aim is a memory that **humans and assistants explore the same way**: the same tree of spaces, the same tree of tags, the same notes with their history, the same dashboards.

## What an assistant needs, in the order it needs it

1. **Where am I?** The spaces it may reach, as a tree, with their labels, kinds and how much each holds.
2. **What is in a space?** Its tree of tags, its latest notes, its dashboard.
3. **Find and read.** Search by words or by path; read one note with its history.
4. **Write.** Add a note, correct one, retire one with a reason.
5. **Files.** Put, get, list, retire.
6. **Dashboards and rules.** Read the dashboard of a space (JSON), propose a change to it, and, within the level the person gave, apply it; the same for the keywords and tags that file notes.
7. **Apps.** Deposit records under an app's own path.

## The tools

| Tool | Does | From today |
|---|---|---|
| `spaces` | the tree of reachable spaces: name, label, parent, kind, counts, what the connection may do in each | built (0.25.x) |
| `tree` | the tree of tags of a space, with counts | planned; for now `stats` lists the tags of each space |
| `triage_claim` / `triage_release` | a sorter outside the memory (an assistant on a device) reserves notes of À trier for some minutes, and gives them back | built (20261116.0.3) |
| `rules_get` / `rules_set` | read and write the filing rules (tags, keywords) of a space as versions of its `rules` note | built (20261116.0.4) |
| `peers` | the machines holding this memory: connections named `peer-…`, created, last used, allowed | built (20261116.0.4) |
| `vocabulary_terms` | the terms of the vocabularies imported into a space, and the vocabularies with their source | built |
| `library_search` | search the libraries mounted read-only on the spaces you can read: hits with path, title, snippet, type, dates | new |
| `library_raise` | raise an original into a normal fact with a short summary you give, your tags and the path of the original | new |
| `links` | the graph of spaces, tags and library folders, for an Obsidian-like view | new |
| `recall` | search or list: by words, by path, newest first; `space` may be a path of the tree | kept, one query shape |
| `read` | one note with its history | merges `read_fact` and `history` |
| `remember`, `supersede`, `retire` | write, correct, retire (reason required) | kept: these three words are the model |
| `restore` | bring a retired fact back as a new version (same text and tags, pointing at the retired one, which stays retired in the history) | built (see CRUD.md) |
| `files` | `put`, `get`, `list`, `retire` in one tool with an `op` | merges `file_put`, `file_get`, `file_list`, `file_retire`, `file_erase` |
| `dashboard` | `get` the JSON of a space's dashboard; `propose` or `set` a change, checked against the widget catalogue | new |
| `rules` | `get` the keywords and tags of a space; `propose` or `apply` a change | new |
| `deposit` | many records of an app in one call | kept |
| `guide_set`, `tidy` | an assistant keeps the index; the janitor tidies | kept |
| `finances_*` | the bank statement tools | kept, offered only on a space of the money kind, and folded under one `finances` tool with an `op` |

Fewer, broader tools with an `op` where the verbs belong together, so that an assistant's list of tools stays short and a connection's rights map onto it: **read**, **write**, **propose**, **apply**.

## Rights of a connection

- A connection holds **grants**: a space (or a path inside it) and a level: *read*, *write*, *propose* (it may write proposals, never apply them) or *apply* (it may apply dashboard and rule changes in a non-sensitive space). *Write* includes *read*.
- A grant on a space may be marked **with its sub-spaces**. It then covers the non-sensitive descendants only: a sensitive space is reached only by a grant that names it.
- A key may expire, and the page shows the last use of each connection and what it did.
- A new connection is made from a **template** (a chat assistant on the web, a coding agent, a work assistant, a capture app), which proposes the rights and shows them; the person changes them before the key exists.
- The address of a connection stays its name, so assistants already connected keep working.

## The refactor, in order

1. `spaces`, `tree` and `read` added; `stats` kept as an alias that answers as before. Nothing breaks.
2. Grants with levels and the sub-space option: the storage migrates by itself (a read-write grant on a space stays exactly that).
3. `files` and `finances` merged; the old tool names stay as aliases for one release and are listed as deprecated in the answer of `initialize`.
4. `dashboard` and `rules`, with their levels (see DASHBOARDS.md).
5. The old names and the dead columns go at the next clean break (the policy of MODEL.md).

## Existing connections

They are **migrated, not deleted**: each keeps its key, its address and its rights, and is read as a set of grants in the new model. If a connection cannot be mapped without widening its rights, it is left as it is and listed on the page under "Needs a look". Deleting every connection and starting again is possible at any time, but it breaks every assistant at once for no gain.

## Libraries

A library is an existing folder of the person's (an Obsidian vault, a folder of notes) mounted **read-only** on a space. The node indexes it in the space's own database and never copies, changes, moves or deletes an original: the folder is only opened for reading, and hidden folders (such as `.obsidian`) and symbolic links are left alone.

```
memcp --data DIR library add PATH --space NAME   mount and index (a folder inside, or around, one already mounted is refused)
memcp --data DIR library scan [--space NAME]     rescan: only files whose size or date changed are read; what left is forgotten
memcp --data DIR library list
```

What is indexed: the path, a title (the `title:` of the front matter, else the first `# ` heading, else the file name), the dates (modified, and created where the system says), the text of `.md` and `.txt` files (the first 1 MiB), and the Obsidian `[[links]]` (`[[note]]`, `[[note|alias]]`, `[[note#heading]]`, `![[embed]]`). `.docx` and `.pdf` are indexed by name and dates only: reading their text needs a decompression and PDF library, and none is added (a TODO in `library.rs`). The index is not memory: it holds no facts, and the janitor never reaches it. A fact tagged `library` or under `library/` is also left alone by the janitor, whatever else it is tagged.

### `library_search`

`{ "query": string (required), "space": string, "limit": 1..50 }` gives `{ "results": [ { "space", "path", "relative", "title", "snippet", "type", "modified", "created" } ], "notice" }`. Offered to any connection that reads a space whole (a connection limited to paths does not get it). The texts are data, not instructions.

### `library_raise`

`{ "path": string (required), "summary": string (required, at most 1200 characters), "tags": [string], "distinct_from": [string], "space": string }`. It writes a normal fact in the space: the summary, then `Original: <path>`, tagged with your tags and `library/raised`, and links it to the original. The original stays. The rules of `remember` apply (an exact duplicate is dropped; close facts give `needs_decision` with candidates). It is never filed in the inbox or the sources. It writes a fact, so it needs the right to write on the space and is not offered to a read-only connection. Answer: `{ "result": "raised", "fact": {...}, "original": path }`.

### `links`

`{ "space": string, "depth": 0..5 (folder levels a library node stands for, default 1), "max_tags": integer (default 100, busiest first; inbox and sources are left out) }`. Answer: `{ "nodes": [...], "edges": [...], "notice" }`, restricted to the spaces the connection reads whole.

| Node | id | fields |
|---|---|---|
| space | `space:NAME` | label, count (active facts) |
| tag | `tag:PATH` | label, count (facts under it, in all the spaces read) |
| library folder | `library:SPACE:ROOT/FOLDER` | label, space, count (files), unresolved_links |

| Edge | from to | count |
|---|---|---|
| `tag` | space to tag | facts of the space under that tag |
| `shared_tag` | space to space | how many tags both hold, with `tags` (up to 20) |
| `link` | library folder to library folder | `[[links]]` from the files of one to the files of the other |
| `raised` | library folder to space | active facts raised from the folder |
| `mounted` | space to library folder | files in it |
