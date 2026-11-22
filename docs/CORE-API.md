# Core API: the map

What `memcp-core` offers, by module. Every public item has a doc comment (`cargo doc -p memcp-core --open`); this page is the map. The crate has no network and no model: it opens files in a data directory (0700; files 0600), one SQLite file per space plus `control.sqlite`.

**Invariants.** Facts are append-only: a fact is never deleted or edited; `supersede` marks the old one `replaced`, `retire` marks it `retired`, both keep the text and a history event. The only erasures are `files::erase` (a voice recording once its transcript exists) and, on a replica that chose a window, `sync::prune_window` (local, never synchronised). The author of a write is the connection's name, never an argument. A function that takes a `Caller` or goes through `tools::call` checks rights; the lower-level functions take an open connection and trust the caller: only the node's HTTP server and the tools may reach them.

**Rights.** A connection holds `read` or `write` per space (`write` includes `read`), or `all`; it can be limited to tag paths inside a space, and forced read-only. Rights never go beyond the person's own access. Column "Rights" below says what a caller needs; "Errors" lists the kinds of the [error vocabulary](#error-vocabulary).

The "Errors" column shows the documented `Error` variants (`Invalid`, `UnknownFact`...); any function that opens a file or a database can also fail with `Io`/`Sqlite` (kind `internal`).

## Space layout and health targets

Space creation and label changes, including layout moves and peer updates, check unique visible labels.
The comparison folds case and accents and trims surrounding spaces. Built-in labels are checked in both
English and French, including when a caller supplies a custom label. Existing duplicates are left in place.
The internal routing queues of different accounts or peers may share the default Inbox label.

`target_set` requires write access to a health space. With a card `id`, omit `problem` to keep its text.
`signs` accepts referential IDs, tags, labels or free text. `factors` accepts free text as favouring factors.
Each supplied array replaces that list. Empty clears it, omitted keeps it. The `targets` view exposes
`signs` and `factors` in each target item, with entries backed by `sign/<slug>` and `factor/<slug>` tags.
Unknown card IDs and non-health spaces are refused. See [HEALTH.md](HEALTH.md).

## `facts`: Facts: write, correct, retire, read, search

Rights: None in the module: the tools check the connection's right (`remember`, `supersede`, `retire` need write on the space; `recall`, `read_fact`, `history` need read). Append-only: no function here erases a fact.

| Function | Purpose | Errors |
|---|---|---|
| `read_fact` | Reads one fact by id, whatever its status, tags included | `Error::Sqlite`/`Io` on storage failure |
| `inbox` | How many active notes of the space wait with the `inbox` tag (or `inbox/…`), and when the oldest was written | `Error::Sqlite`/`Io` on storage failure |
| `normalize_tag` | A tag in its one accepted form, or an error that says what is accepted (an assistant reads it and tries again) | `Error::Invalid` (says what is accepted) |
| `normalize_tags` | The tags of a fact, each normalised, without repeats, in order | `Error::Invalid` on the first bad tag |
| `tag_counts` | The tags in use on active facts, with how many facts each covers | `Error::Sqlite`/`Io` on storage failure |
| `remember` | Writes a new fact in `space` on behalf of `author` | `Error::Invalid` for an empty text, control characters, an unknown kind, a bad tag, or `valid_to` before `valid_from`. |
| `active_digests` | Every active fact of a space: id, text digest, text | `Error::Sqlite`/`Io` on storage failure |
| `close_pairs` | The pairs of active facts at or above `least`, most alike first, each pair once | `Error::Sqlite`/`Io` on storage failure |
| `tidy_after` | The tidying that goes with each write (5S): what the memory noticed around a fact just written | `Error::Sqlite`/`Io` on storage failure |
| `supersede` | Replaces the active fact `old_id` with a corrected version, written by `author` | `Error::UnknownFact` if `old_id` is not in `space`; `Error::Invalid` if the old fact is not `active`, the new text is identical (same tags) or already active as another fact, or `new` breaks the rules of `remember` (text, ... |
| `retire` | Retires an `active` or `proposed` fact: it leaves default searches but its text stays and can be read by id and in `history` | `Error::Invalid` (empty reason, unknown file) |
| `history` | The replacement chain around a fact, from the oldest version to the newest, whatever the status of each version | `Error::UnknownFact` if `id` is not in this database. |
| `recall` | Lexical search (FTS5, BM25) on stems | `Error::Invalid` for a malformed tag in `opts`.  A query with no word to search for (empty, or only words too common to count) and at least one tag is a **listing**: what is filed under those tags, newest first. It is how an ... |
| `counts` | Number of facts per status, as a JSON object (`{"active": 12, "retired": 1}`; absent statuses are not listed) | `Error::Sqlite`/`Io` on storage failure |
| `audit` | Appends one row to the space audit log: who (`client`) called which `tool`, whether it worked, the ids of the facts touched, a hash of the arguments and the first 300 characters of `detail` | none (pure) |

## `files`: Files kept in a space

Rights: Tools: `file_put`, `file_retire`, `file_erase` need write; `file_get`, `file_list` need read. Only `erase` ever removes bytes.

| Function | Purpose | Errors |
|---|---|---|
| `sniff` | The type of the content, from its first bytes | none (pure) |
| `put` | Stores a file in a space and describes it with a fact | `Error::Invalid` (empty, no name, type not kept); `Io` if the bytes cannot be stored |
| `put_tagged` | `put`, with the tags (already normalised) the describing fact is filed under: an app that deposits a file files it under `sources/<app>` | as `put` |
| `info` | The record of a file by id, any status | none beyond storage failure |
| `get` | The record and the bytes of a file | `Error::Schema` if the bytes are damaged; `None` if unknown or erased |
| `list` | Active files of a space, newest first, optionally matching words of the name or note | none (pure) |
| `retire` | Retires a file: out of lists and searches, the bytes stay on disk | `Error::Invalid` (empty reason, unknown file) |
| `erased` | The recording is gone for good (see `erase`); its record stays | `Error::Sqlite`/`Io` on storage failure |
| `erase` | Erases a voice memo for good, once the text that replaces it is written | `Error::Invalid` (not a recording, transcript missing or not yet written) |
| `erase_at` | `erase` at a given instant | as `erase` |

## `access`: Connections, rights and tokens

Rights: `connect*`, `regrant`, `revoke`, `rename`, `renew`, `set_*`: the owner of the connection or an administrator (the node checks). `authenticate*`: anyone holding a token.

| Function | Purpose | Errors |
|---|---|---|
| `parse_spec` | Parses `health:rw,work:r`, and `health/sources/bevel:rw` for a grant limited to a tag path inside a space | `Error::Invalid` for a malformed value |
| `hash_token` | SHA-256 of a token: the only form in which a token is stored and compared | none (pure) |
| `valid_agent` | Whether `name` is a valid connection name: 1 to 40 lowercase letters, digits or dashes, not starting with a dash | none (pure) |
| `space_of_kind` | The space of a kind that this caller can read: the one its person owns if there is one, else the first by name | `Error::Sqlite`/`Io` on storage failure |
| `open_space_connection` | Opens one space to an assistant: a connection limited to that space, with no nothing else, named after the space | `Error::Sqlite`/`Io` on storage failure |
| `connect` | `connect_for` the bootstrap administrator | `Error::Sqlite`/`Io` on storage failure |
| `connect_for` | Creates a connection named `agent` for the person `user_id` and returns its token (shown once; only its hash is kept) | `Error::Invalid` for a bad name, a name that is a space's name or another person's, no space requested, a grant on a space the person does not reach (a connection never goes beyond its person), an unknown or disabled person, or ... |
| `renew` | A new token for an active connection of this person, rights and read-only setting unchanged | `Error::Sqlite`/`Io` on storage failure |
| `owner_of` | The person an active connection belongs to | `Error::Sqlite`/`Io` on storage failure |
| `rename` | Gives a connection another name, one a person can read in a list | `Error::Sqlite`/`Io` on storage failure |
| `regrant` | Changes the rights of an active connection without touching its token: the assistant or device connected has nothing to redo | `Error::Sqlite`/`Io` on storage failure |
| `authenticate` | Identifies the caller by its token | `Error::Sqlite`/`Io` on storage failure |
| `authenticate_console` | The person behind a console token (the node page), or `None` | `Error::Sqlite`/`Io` on storage failure |
| `console_token` | Creates (or renews) the console token of a person: it opens the node page, never MCP | `Error::Sqlite`/`Io` on storage failure |
| `touch` | Records that `caller` was just used (time and address), at most once a minute per connection (fewer writes on the disk) | `Error::Sqlite`/`Io` on storage failure |
| `revoke` | Revokes every active key of the connection `agent` and its OAuth refresh tokens; it can never be brought back (make a new connection) | `Error::Sqlite`/`Io` on storage failure |
| `set_no_person_guide` | Whether the person's own guide is left out of what `agent` is told when it connects | `Error::Sqlite`/`Io` on storage failure |
| `set_read_only` | Forces (`on`) or releases the read-only mode of `agent` | `Error::Sqlite`/`Io` on storage failure |
| `set_limit` | `key`: `rate` (requests/minute) or `max-text` (characters) | `Error::Sqlite`/`Io` on storage failure |
| `list` | Every connection (consoles excluded), oldest first | `Error::Sqlite`/`Io` on storage failure |
| `auth_event` | A refused authentication or tool call, for the control audit | none (pure) |

## `users`: People and the spaces they own

Rights: Administrators create and disable people; a person manages their own display, heir and recovery key.

| Function | Purpose | Errors |
|---|---|---|
| `create_user` | Creates a person | `Error::Invalid` for a bad `name` or a name already taken. |
| `user` | A person by id (also a disabled one); `Ok(None)` if unknown | `Error::Sqlite`/`Io` on storage failure |
| `user_by_name` | A person by login name (also a disabled one); `Ok(None)` if unknown | `Error::Sqlite`/`Io` on storage failure |
| `resolve` | The id of an active user given by id or name | `Error::Sqlite`/`Io` on storage failure |
| `list_users` | Every person, oldest first, disabled ones included | `Error::Sqlite`/`Io` on storage failure |
| `set_admin` | Changes the role | `Error::Sqlite`/`Io` on storage failure |
| `set_display` | Changes the display name of the person `id` | `Error::Sqlite`/`Io` on storage failure |
| `set_disabled` | Disables a person: their connections stop working at once | `Error::Sqlite`/`Io` on storage failure |
| `from_home_assistant` | The person behind a Home Assistant login, created as a member on first sight (or as an administrator when listed in `admins`) | none (pure) |
| `claim_owner` | Turns the bootstrap user of a fresh data directory into a person: a name, a display name, the Home Assistant login it answers to, and a plain member (an installation's administrators live in the registry of whoever holds several people, never in a person's own directory) | none (pure) |
| `set_ha_user` | Points an existing person at another Home Assistant login (or none) | `Error::Sqlite`/`Io` on storage failure |
| `ha_login` | Like `from_home_assistant`, except that a login which would become a new member does not: it is recorded as a request and waits for an administrator | none (pure) |
| `list_requests` | The logins waiting for, or refused by, an administrator: refused ones first, then pending ones, each by first sighting | `Error::Sqlite`/`Io` on storage failure |
| `request_status` | `pending` or `refused`, if this login has a request | `Error::Sqlite`/`Io` on storage failure |
| `approve_request` | An administrator approves a pending login: the person is created (a member, or an administrator if asked), linked to the login, and the request is removed | `Error::Sqlite`/`Io` on storage failure |
| `refuse_request` | An administrator refuses a pending login | `Error::Sqlite`/`Io` on storage failure |
| `is_space_owner` | Whether `user_id` owns `space` | `Error::Sqlite`/`Io` on storage failure |
| `access_of` | Every space a person reaches: the ones they own, with the right to write | `Error::Sqlite`/`Io` on storage failure |
| `set_heir` | Names the person who may take over (or none) | `Error::Sqlite`/`Io` on storage failure |
| `heir_of` | The person `id` named as their heir, if any | `Error::Sqlite`/`Io` on storage failure |
| `heir_to` | The people who named this person their heir | `Error::Sqlite`/`Io` on storage failure |
| `touch_seen` | Notes that the person opened their page (at most once a minute) | `Error::Sqlite`/`Io` on storage failure |
| `last_seen` | When the person was last seen, or when they were made if never | `Error::Sqlite`/`Io` on storage failure |
| `make_recovery_key` | Makes a recovery key for the person (replacing any earlier one) and returns it: shown once, only its digest is kept | `Error::Sqlite`/`Io` on storage failure |
| `has_recovery_key` | Whether a recovery key was made for the person (the key itself is never stored, only its digest) | `Error::Sqlite`/`Io` on storage failure |
| `claim_with_recovery_key` | A Home Assistant account that holds a recovery key takes over the person it belongs to | `Error::Sqlite`/`Io` on storage failure |
| `attach_request` | The heir of `person` attaches a waiting Home Assistant account to them, when the person has not been seen for `HEIR_WAIT_SECS` | none (pure) |

## `tools`: The MCP tools an assistant sees

Rights: `definitions` and `call` filter by the connection's rights; see TOOLS.md.

| Function | Purpose | Errors |
|---|---|---|
| `definitions` | The tools this connection sees, filtered by its rights: nothing needing write is listed for a read-only connection, and a connection with no readable space gets none | none (pure) |
| `call` | Runs a tool | `Error::Sqlite`/`Io` on storage failure |

## `triage`: The Inbox of a person

Its label is « Inbox »; it is a space of kind `a-trier` named `<person>-a-trier` (`alice-a-trier`), the name it always had, so a box made by an earlier version is the same box.

Rights: Owner of the box (page) or a connection holding write on it.

| Function | Purpose | Errors |
|---|---|---|
| `of` | The person's Inbox, if it was made | `Error::Sqlite`/`Io` on storage failure |
| `ensure` | The person's Inbox, made now if they have none | `Error::Sqlite`/`Io` on storage failure |
| `is_triage` | Whether `space` is an Inbox (kind `a-trier`) | `Error::Sqlite`/`Io` on storage failure |
| `waiting` | What waits in an Inbox: its notes (a recording's own description apart: the node reads the recording and puts its text here) and its files, newest first | `Error::Sqlite`/`Io` on storage failure |
| `send` | Sends one note or one file of `from` (an Inbox) to the space `to`, tagged `inbox`, as it is; then it leaves `from`, once the write is certain | `Error::Sqlite`/`Io` on storage failure |

## `sync`: Peer synchronisation: the log, applying events, cursors, the window

Rights: A connection named `peer-...` (the node checks); pruning is local and only on a replica with a window.

| Function | Purpose | Errors |
|---|---|---|
| `stamp` | A new stamp, after every one this copy has issued or seen | `Error::Sqlite`/`Io` on storage failure |
| `head` | The last position of the space's log | `Error::Sqlite`/`Io` on storage failure |
| `events_after` | The events after position `after`, at most `limit` and about `max_bytes`, without the ones `exclude_via` delivered | none (pure) |
| `disputes` | Facts in dispute that hold (at least two alternatives stand): the fact and what was done to it differently (the ids of its replacements, `retired`) | `Error::Sqlite`/`Io` on storage failure |
| `open_disputes` | The same, each alternative with its text, author, date or reason (`Dispute`, `Alternative`) | `Error::Sqlite`/`Io` |
| `resolve` | Keeps one alternative of a dispute: the others are retired with `dispute resolved: kept …` and a `dispute_resolved` event is logged | `Error::Invalid` when the fact is not in dispute or `keep` is not one of its alternatives |
| `dispute_marks`, `dispute_count` | The facts of each dispute (to flag them), and how many disputes hold | `Error::Sqlite`/`Io` |
| `apply` | Applies events received from the peer `via`, in order | `Error::Sqlite`/`Io` on storage failure |
| `apply_window` | `apply` for a replica that holds a window: with `cutoff` (Unix seconds, see `window_cutoff`) a note created before it is left out unless it is pinned, and so are its files and what is said about it later | none (pure) |
| `cursors` | How far this node has read `peer`'s log of `space`, and how far `peer` has read this one's | `Error::Sqlite`/`Io` on storage failure |
| `set_cursor` | Moves one cursor of `peer` on `space` to `to` (created at 0 on first use) | `Error::Sqlite`/`Io` on storage failure |
| `note_transfer` | Notes that `peer` moved events of `space`: the direction as this node sees it (`sending`: its events leave, `receiving`: the peer's arrive), how far it got and how far there is to go | none (pure) |
| `machine_name` | `peer-macbook-ab12` is the machine `macbook` | none (pure) |
| `status` | Every peer: those that hold a connection here and those this node syncs with, what each moved last and when | `Error::Sqlite`/`Io` on storage failure |
| `ensure_space` | Makes the space on this node when a peer has it and this node does not (the first join is a full copy) | `Error::Sqlite`/`Io` on storage failure |
| `window_days` | The window of this replica in days, or `None` (the default): it holds everything | `Error::Sqlite`/`Io` on storage failure |
| `set_window_days` | Sets the window (`Some(days)`, 1 to 36500) or turns it off (`None`) | `Error::Sqlite`/`Io` on storage failure |
| `window_cutoff` | The instant (Unix seconds) before which a note is outside the window, or `None` without a window | `Error::Sqlite`/`Io` on storage failure |
| `prune_window` | Removes from this replica, for good, the notes of `space` created more than `days` days before `now`, with their tags, history, files and log rows, but only those that every peer this replica syncs with has acknowledged, that are not pinned (`pin`, `pin/…`) and that no kept note points to | `Error::Sqlite`/`Io` on storage failure |
| `window_status` | The window of this replica and what the last pruning did | `Error::Sqlite`/`Io` on storage failure |
| `prune_all` | Prunes every space of this replica to its window, if it has one, and remembers what was done | `Error::Sqlite`/`Io` on storage failure |

## `layout`: The tree of spaces as a person arranges it

Rights: The owner of the spaces.

| Function | Purpose | Errors |
|---|---|---|
| `layout` | The spaces a person owns, in tree order: top level first by position, each followed by its children | `Error::Invalid` for a malformed value |
| `set_layout` | Renames, moves or reorders one of a person's spaces | `Error::Invalid` for a malformed value |
| `add_custom_space` | Creates a space of a person's own, of kind `custom`, with a label and perhaps under a parent | none (pure) |

## `lifecycle`, `rules`, `peers`, `people`: create, update and delete what a person owns

The matrix of every object and operation, with the command line, the HTTP route, the page button and the MCP tool, is `docs/CRUD.md`.

Rights: The owner of the objects (an administrator for `people`).

| Function | Purpose | Errors |
|---|---|---|
| `Brain::create_space_with` | Creates a space of a kind, with a label, a parent and keywords | `Error::Invalid` for an unknown kind or a malformed value |
| `Brain::space_info` | Reads one of a person's spaces: label, kind, parent, counts, rules | `Error::Invalid` for a space that is not theirs |
| `Brain::update_space` | Changes label, parent, position, kind, keywords and tags | `Error::Invalid` for a malformed value |
| `Brain::delete_space` | Deletes a space with a choice (into another, or archive), with a dry run; never destroys | `Error::Invalid` for a space that is not theirs, the Inbox, a bad choice |
| `Brain::archived_spaces`, `Brain::restore_space` | Lists the archive; brings an archived space back | `Error::Invalid` for an unknown archive or a name in use |
| `rules::read`, `rules::write`, `rules::edit` | The keywords, tags and source apps of a space, one new version of the `rules` note per change | `Error::Invalid` for a malformed tag |
| `peers::list`, `peers::get`, `peers::update`, `peers::set_paused`, `peers::revoke`, `peers::forget` | A person's machines: read, rename and note, pause and resume, revoke, forget | `Error::Invalid` for a machine that is not theirs or still in use |
| `people::retire_person` | Removes a person = disables them and exports their memory | `Error::Invalid` for a non-administrator, oneself, the last administrator |

## `merge`: Merging another memory's export into this one

Rights: An administrator, from the command line; a backup is made first.

| Function | Purpose | Errors |
|---|---|---|
| `plan` | Reads an export and proposes, for each of its spaces, what to do | `Error::Sqlite`/`Io` on storage failure |
| `preview` | Fills in the preview and the guide state of every line, from what is there now | `Error::Sqlite`/`Io` on storage failure |
| `apply` | Applies a plan: backup, then every line in one transaction per target space, then the verification of the counts, then the commit | `Error::Sqlite`/`Io` on storage failure |

## `library`: Read-only libraries mounted on a space

Rights: Mounting and scanning: an administrator. `search` needs read; `raise` needs write.

| Function | Purpose | Errors |
|---|---|---|
| `add_root` | Mounts `dir` on this space | `Error::Sqlite`/`Io` on storage failure |
| `roots` | The folders mounted on this space | `Error::Sqlite`/`Io` on storage failure |
| `parse_links` | The ``[targets`]` of a Markdown text: ``[note`]`, ``[note\|alias`]`, ``[note#heading`]` and `!`[embed`]` give `note` (a link stays on one line) | none (pure) |
| `rescan` | Reads the folder again and brings the index in line with it: new and changed files are read, the others are skipped, and what is gone is forgotten (a fact raised from it stays) | `Error::Sqlite`/`Io` on storage failure |
| `rescan_all` | Rescans every folder mounted on this space | `Error::Sqlite`/`Io` on storage failure |
| `search` | Looks the index up by words (accents and plurals ignored) | `Error::Sqlite`/`Io` on storage failure |
| `raise` | Raises an original: writes a normal fact in `space` with the caller's `summary` and `tags`, and links it to the original, which stays where it is | none (pure) |
| `graph` | The folders of this space's libraries (cut at `depth` levels), the ``[links`]` between them, and the facts raised from them | `Error::Sqlite`/`Io` on storage failure |

## `vocabulary`: Imported vocabularies of a space

Rights: Import and removal: the owner; reading: read on the space.

| Function | Purpose | Errors |
|---|---|---|
| `clean_term` | A term as the router compares it: lower case, no leading `#`, spaces folded | none (pure) |
| `parse` | Reads a plain file: one term per line, or `term<TAB>group<TAB>code`; blank lines and `#` comments are skipped | none (pure) |
| `import` | Imports (replaces) the vocabulary `name`; the number of terms kept | none (pure) |
| `list` | The vocabularies of a space | `Error::Sqlite`/`Io` on storage failure |
| `remove` | Removes a vocabulary and its terms; whether it existed | `Error::Sqlite`/`Io` on storage failure |
| `terms` | Every distinct term of the space's vocabularies, up to `limit`, in alphabetical order | `Error::Sqlite`/`Io` on storage failure |

## `pairing`: Joining a device with a one-time code

Rights: The person makes the code on their page; the device holds the code (ten minutes, once).

| Function | Purpose | Errors |
|---|---|---|
| `normalize` | A code as the person may type it: lower case, with a dash or spaces | none (pure) |
| `display` | `ABCD-EFGH`, the way it is shown | none (pure) |
| `rights_for` | The rights a device gets for the spaces ticked: writing with the `inbox` tag in each, nothing else | `Error::Invalid` for a malformed value |
| `create` | A new code for `user_id`, valid `CODE_TTL` seconds from `now`, for the `inbox` tag of the spaces `vaults` | `Error::Sqlite`/`Io` on storage failure |
| `create_for_peer` | A new code for a node of the person's own (a peer): it reads and writes every space the person owns, as this node does | none (pure) |
| `standing` | Where the code `code` made by `user_id` stands at time `now` | `Error::Sqlite`/`Io` on storage failure |
| `holds` | Whether this control database holds `code`, unused and not expired (the manager finds the person this way) | `Error::Sqlite`/`Io` on storage failure |
| `redeem` | Exchanges `code` for a new connection, once | `Error::Sqlite`/`Io` on storage failure |

## `export`: Exporting and adopting whole spaces

Rights: Export: the owner or an administrator. Adopt: an administrator.

| Function | Purpose | Errors |
|---|---|---|
| `export_person` | Writes the export of a person into `<to>/memcp-export-<person>-<time>/` and returns that directory | `Error::Sqlite`/`Io` on storage failure |
| `verify_export` | Checks an export: the manifest, the fingerprint of every file, and that nothing else is there | `Error::Invalid` for a malformed value |
| `adopt` | Takes the spaces of an export into this installation as the property of `owner` | none (pure) |

## `guide`: Guides and indexes written for assistants

Rights: Guides: written only by a person on the page, no tool can; app guides: proposed by an app, approved by a person.

| Function | Purpose | Errors |
|---|---|---|
| `person_scope` | The scope of a person's own guide: `person:<id>` | none (pure) |
| `space_scope` | The scope of a space's guide: `space:<name>` | none (pure) |
| `current` | The text now in force for a scope, if any | `Error::Sqlite`/`Io` on storage failure |
| `history` | Every text this scope has had, newest first (the one in force first, if there is one) | `Error::Sqlite`/`Io` on storage failure |
| `set` | Writes the guide of a scope | `Error::Sqlite`/`Io` on storage failure |
| `clear` | Takes the guide of a scope out of force | `Error::Sqlite`/`Io` on storage failure |
| `instructions` | What a connection is told when it connects: the general rules, then the guide of the person and of each space it reaches (instructions, written by a person), then the index of each of those spaces (data, written by assistants) | `Error::Sqlite`/`Io` on storage failure |
| `propose_app_guide` | An app proposes (or replaces) its guide for a space | `Error::Sqlite`/`Io` on storage failure |
| `app_guides` | The guides the apps proposed in one space, with where each stands for this person | `Error::Sqlite`/`Io` on storage failure |
| `approve_app_guide` | Approves a guide as it reads now | none (pure) |
| `withdraw_app_guide` | Takes an approval back: the guide is data again | `Error::Sqlite`/`Io` on storage failure |

## `origin`: Records an app deposits under its own ids

Rights: Write on the space (or the app's path).

| Function | Purpose | Errors |
|---|---|---|
| `deposit` | Deposits a fact under the app's own id | none (pure) |
| `cursor` | The cursor of one source: its latest data point and how many facts of it are active | `Error::Sqlite`/`Io` on storage failure |
| `cursor_in` | The cursor of one app in one layer | `Error::Sqlite`/`Io` on storage failure |
| `sources` | Every source that deposited here, with its cursor | `Error::Sqlite`/`Io` on storage failure |
| `dossiers` | Every app that wrote into the dossier layer, with its cursor | `Error::Sqlite`/`Io` on storage failure |

## `import`: Importing a folder of notes as passages

Rights: An administrator, from the command line.

| Function | Purpose | Errors |
|---|---|---|
| `date_from_name` | Date of a note dated by its name (`2026-10-30.md`, `2026-10-30 Meeting.md`), in Unix seconds at UTC midnight: a journal or daily note holds from that day on | none (pure) |
| `chunk` | Cuts a note into passages | none (pure) |
| `looks_secret` | True if the text looks like a secret: private key, `password: value`, `Bearer` token, long alphanumeric run mixing letters and digits | none (pure) |
| `import` | Reads a folder of notes into passages (facts of kind `passage`, source `note`), routed to spaces by `rules` | `Error::Invalid` if `vault` (the folder of notes) is not a folder, a space in the rules does not exist, or a real first run has no recent dry run. |

## `catalogue`: The kinds of space an installation offers

Rights: None (pure).

| Function | Purpose | Errors |
|---|---|---|
| `default_catalogue` | What a person needs to keep track of: themselves, their health, the people around them, their home and daily life, their money, their work, their passions | none (pure) |
| `parse` | A catalogue from text | `Error::Invalid` for a malformed value |
| `vault_name` | The name of the space of `kind` that a person owns: `<person>-<kind>`, the person's name shortened to fit the 32 characters of a space name | `Error::Invalid` for a malformed value |

## `brain`: The data directory and the control database

Rights: Whoever can open the directory (it is 0700).

| Function | Purpose | Errors |
|---|---|---|
| `default_config` | Configuration written by `init` | none (pure) |
| `init` | Creates the directory (0700), the control database, the given spaces and the default configuration (files in 0600) | `Error::Sqlite`/`Io` on storage failure |
| `open` | Opens an initialised directory (it must hold `control.sqlite`) | `Error::Sqlite`/`Io` on storage failure |
| `dir` | The data directory this handle points at (the folder given to `init` or `open`) | none (pure) |
| `language` | The search language set in the configuration (English if absent) | `Error::Invalid` for a malformed value |
| `spaces` | Registered spaces, in name order | `Error::Invalid` for a malformed value |
| `has_space` | Whether `space` is registered in the control database | `Error::Invalid` for a malformed value |
| `catalogue` | The kinds of space this installation offers: its `catalogue.txt` if it has one, the default for a person's life otherwise | `Error::Invalid` for a malformed value |
| `space_kind` | The kind of a space (`custom` when it was made without one) | `Error::Invalid` for a malformed value |
| `set_space_kind` | Gives a space a kind of the catalogue (or back to `custom`) | `Error::Invalid` for a malformed value |
| `add_vault` | Creates a person's space of a kind of the catalogue, named after the person and the kind | `Error::Invalid` for a malformed value |
| `add_space` | Creates a new space owned by the bootstrap administrator | `Error::Invalid` for a malformed value |
| `add_space_for` | Creates a new space (its file and its registration) owned by `user_id`, who gets read and write access | `Error::Invalid` for a malformed value |
| `open_space` | Opens a space file, applies missing migrations and checks that the file declares this space (a file stored under the wrong name is refused) | `Error::Sqlite`/`Io` on storage failure |
| `open_control` | Opens the control database (tokens, rights, spaces, authentication audit) | `Error::Sqlite`/`Io` on storage failure |
| `backup` | Consistent copy of every database (`VACUUM INTO`, safe while the node writes) and of the configuration into `root/backup-YYYY-MM-DDTHHMMSS/`, then keeps only the `keep` most recent ones | `Error::Invalid` for a malformed value |

## `text`: Normalising, stemming, hashing

Rights: None (pure).

| Function | Purpose | Errors |
|---|---|---|
| `set_language` | Sets the process language | none (pure) |
| `language` | The process language: the one given to `set_language`, English if none was | none (pure) |
| `normalize` | NFC, whitespace collapsed to single spaces, trimmed | none (pure) |
| `text_hash` | Hex SHA-256 of the normalised text | none (pure) |
| `hex` | Lowercase hexadecimal text of `bytes` (two characters per byte) | none (pure) |
| `fold` | Lowercase, accents removed | none (pure) |
| `stems` | Stems of a text in the process language | none (pure) |
| `stems_in` | Stems of a text: folded, split on non-alphanumerics, stopwords removed, stemmed | none (pure) |
| `similarity` | Jaccard index of two stem sets: 0 (nothing shared) to 1 (same words) | none (pure) |

## `util`: Clock and random bytes

Rights: None (pure).

| Function | Purpose | Errors |
|---|---|---|
| `now` | Seconds since the Unix epoch (0 if the clock is before it) | none (pure) |
| `now_ms` | Milliseconds since the Unix epoch | none (pure) |
| `random_bytes` | `N` random bytes from the OS | none (pure) |
| `random_base32` | `n` random lowercase base32 characters | none (pure) |
| `new_fact_id` | Public identifier of a fact: `f_` + 12 characters | none (pure) |
| `civil` | UTC (year, month, day, hour, minute, second) of a Unix timestamp (Howard Hinnant's algorithm) | none (pure) |
| `compact` | `YYYY-MM-DDTHHMMSS` (UTC), used in file and upload names | none (pure) |
| `iso` | `YYYY-MM-DDTHH:MM:SS+00:00` (UTC) | none (pure) |
| `ct_eq` | Constant-time comparison of two digests | none (pure) |

## Error vocabulary

Every failure has exactly one kind (`ErrorKind`). `Error::kind()` and `ToolError::kind()` give it; a test (`crates/core/tests/error_kinds.rs`) drives the error paths of the tools and of the core and checks that each one answers with a kind of this table, with the status written here.

| Kind (`code`) | HTTP | MCP text (`isError: true`) | Raised by |
|---|---|---|---|
| `invalid` | 400 | the message itself, in words an assistant can act on (`"text" (text) is required`, `unknown fact`) | `Error::Invalid`, `ToolError::Invalid` |
| `unauthorized` | 401 | none: refused before MCP (`refused`) | the node, on a missing, revoked or expired key |
| `denied` | 403 | `no read right on <space>`, `no write right on <space>`, `this connection writes only under <paths>` | `ToolError::Denied` |
| `not_found` | 404 | the message (`unknown fact: <id>`, `unknown space: <name>`); through MCP an unknown fact is reported as `invalid` `unknown fact`, so that a fact of a space you cannot read is not told apart from a missing one | `Error::UnknownFact`, `Error::UnknownSpace` |
| `conflict` | 409 | the message (`<dir> is already initialised, nothing was changed`) | `Error::AlreadyInitialized` |
| `too_large` | 413 | none: the node refuses the body | the node |
| `limited` | 429 | `text too long for this connection (n > max characters)`; HTTP: `too many attempts` with `Retry-After` | `ToolError::Limited`, the node's rate limit |
| `internal` | 500 | `internal error: <detail>` | `Error::Io`, `Error::Sqlite`, `Error::Schema`, `Error::DirectoryTooOpen`, `ToolError::Internal` |
| `unavailable` | 503 | the message (`<dir> is not initialised (run \`memcp init\`)`) | `Error::NotInitialized` |
| `unknown_tool` | 200 | JSON-RPC error `-32602`, `Unknown tool: <name>` (a hidden tool answers like a missing one) | `ToolError::Unknown` |

## Main flows

The crate-level example of `memcp_core` (doc-test, run by `cargo test --doc`) shows remember, recall, supersede and retire. `crates/core/tests/flows.rs` runs the other flows end to end: a triage claim, the filing rules, and joining then syncing with a peer.
