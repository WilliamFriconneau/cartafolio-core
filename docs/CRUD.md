# Create, read, update, delete: what a person can do with what they own

A person owns spaces, notes, files, keywords, machines (nodes), connections (assistants) and, for an administrator,
other people. For each object this page says where each of the four operations lives: the core function (library),
the command line (`memcp --data DIR ...`), the HTTP route of the node page (`/admin/api/...`), a button of the page
and, where it is safe for an assistant to do it, an MCP tool.

Rules that hold for every row:

- **Nothing is destroyed silently.** A delete is a retirement, a move, an archive or an export first. History stays.
- **Preview first.** Every destructive HTTP route takes `"dry_run": true` and answers what it *would* do, without doing
  it; the command line has `--dry-run`. The page asks the question, shows that preview and waits for a confirmation
  built into the page (a hold-to-confirm dialog, never a browser dialog).
- **Audited.** Every create, update and delete of a space, node, connection or person writes a line in the control
  audit (`auth_audit`: who, what, which object). Notes keep their own history.
- **Own objects only.** A member touches their own objects. An administrator manages people and access, not other
  people's content. A route that names someone else's object answers 403 or 404, never a result.
- **Assistants** get tools only for what is safe: writing and correcting notes, restoring a retired note, the keywords
  of a space they may write. Creating or deleting spaces, nodes, connections and people stays with the person.

- **By key, from anywhere.** Every command of the **CLI** column also runs against a running node with a console key
  (`memcp --node ADDRESS --key-file FILE ...`, [CLI.md](CLI.md)): the node runs the same function with the key's
  person's rights and answers the same text, so each row below is reachable by key without its own route. The
  routes of the **HTTP** column stay the way for a page or an app.

Legend: **core** = `memcp_core` function, **CLI** = `memcp` command, **HTTP** = route under `/admin/api`,
**page** = button on the node page, **MCP** = tool for the person's assistants. `-` = deliberately not offered.

## Spaces

| Op | core | CLI | HTTP | page | MCP |
|---|---|---|---|---|---|
| Create (kind, label, parent, keywords) | `Brain::create_space_with` | `space new LABEL [--kind K] [--parent S] [--keywords a,b]` | `POST /spaces/create` (and `/layout/add`) | "New space" form, "+" on a folder | - |
| Read | `Brain::layout`, `Brain::space_info` | `space tree`, `space show NAME` | `GET /layout`, `GET /space`, `GET /spaces/one` | the tree, the space view | `spaces` |
| Update: label, parent, position | `Brain::set_layout` | `space set NAME --label L --parent S --position N` | `POST /layout/set` | rename, drag and drop | - |
| Update: kind, keywords, tags | `Brain::update_space` | `space set NAME --kind K`, `rules ...` | `POST /spaces/update`, `POST /space/rules` | kind selector, the keywords block | `rules_get`, `rules_set` |
| Delete: into another space | `Brain::delete_space` (`Into`) | `space delete NAME --into S [--children ...] [--dry-run]` | `POST /spaces/delete` `{into}` | "Delete" > "Move everything into..." | - |
| Delete: archive | `Brain::delete_space` (`Archive`) | `space delete NAME --archive` | `POST /spaces/delete` `{archive: true}` | "Delete" > "Archive" | - |
| Restore an archived space | `Brain::restore_space`, `Brain::archived_spaces` | `space archived`, `space restore ARCHIVE` | `GET /spaces/archived`, `POST /spaces/restore` | "Archived spaces" list | - |

A space is never destroyed silently. `delete` needs a choice. **Into**: every active note, file and tag goes to the
other space through the merge engine (nothing is deleted, merging twice changes nothing); the keywords and tags of the
space join the target's; then the space is archived like below, so its history is kept. **Archive**: a full export
(readable Markdown, the complete database, the files, checked) is written to `archive/<space>-<time>/export` in the
data directory (`MEMCP_ARCHIVE` moves it), the live database and its files are moved aside, not erased, into
`archive/<space>-<time>/live`, and the space leaves the layout. `restore` moves them back and registers the space
again with its label, kind and place in the tree. A space with children asks what to do with them: they go to the
grandparent by default (`--children grandparent|top|under:SPACE`). The preview lists the notes, files, children and the
connections that lose access.

## Nodes (machines that hold this memory)

| Op | core | CLI | HTTP | page | MCP |
|---|---|---|---|---|---|
| Create = join | `pairing`, join flow | `connect peer-...` (on the node's own command line only), `memcp-sync --join` | `POST /join-memory`, `POST /join/...` | "Join a memory", the join banner | - |
| Read (list, state) | `peers::list` | `node list` | `GET /nodes`, `GET /sync/status` | the machines list | `peers` |
| Update: name, note | `peers::update` | `node rename PEER NAME`, `node note PEER TEXT` | `POST /nodes/update` | rename and note on the machine | - |
| Pause, resume sync | `peers::set_paused` | `node pause PEER`, `node resume PEER` | `POST /nodes/pause`, `POST /nodes/resume` | pause / resume button | - |
| Revoke | `peers::revoke` | `node revoke PEER` (or `revoke`) | `POST /nodes/revoke` | "Revoke" | - |
| Forget (remove from lists) | `peers::forget` | `node forget PEER` | `POST /nodes/forget` | "Forget" on a revoked or silent node | - |

A node is a machine connection (kind `peer`, made by a pairing code, a join, or `memcp connect peer-…` on the node's own command line), named `peer-<machine>-xxxx`; the name alone makes nothing a machine. Revoking stops its key at once; the entry stays, marked
revoked, with its history. Forgetting only hides a revoked node, or one silent for 30 days, from the lists (its sync
progress rows go); it comes back by itself if it speaks again. A paused node is refused by this node's sync routes
(and not contacted by this node's own sync pass) until resumed; nothing is lost, it resumes where it stopped.
The list of what each node exposes comes with the peer-directory work and is not part of this page.

## Connections (assistants)

| Op | core | CLI | HTTP | page | MCP |
|---|---|---|---|---|---|
| Create | `access::connect_for` | `connect AGENT ... [--internal]`, `open VAULT` | `POST /connections`, `POST /spaces/open` | "Connect an assistant" | - |
| Read | `access::list`, `access::get` | `connections`, `connection show AGENT` | `GET /state`, `GET /connections/one` | the connections list | - |
| Update: rename | `access::rename` | `connection rename AGENT NAME` | `POST /connections/rename` | rename | - |
| Update: kind (assistant or internal tool) | `access::set_internal` | `internal AGENT on|off`, `connect --internal` | `POST /connections/internal` | "mark as internal tool" | - |
| Update: kind (assistant or internal tool) | `access::set_internal` | `internal AGENT on\|off`, `connect --internal` | `POST /connections/internal` | "mark as internal tool", "treat as an assistant" | - |
| Update: rights | `access::regrant` | `regrant AGENT ...` | `POST /connections/regrant` | rights editor | - |
| Rotate key | `access::rotate` | `connection rotate AGENT` | `POST /connections/rotate` | "New key" | - |
| Revoke | `access::revoke` | `revoke AGENT` | `POST /connections/revoke` | "Revoke" | - |
| Delete (a revoked one) | `access::delete` | `connection delete AGENT` | `POST /connections/delete` | "Remove from the list" | - |

A connection is an **assistant**, a **peer** (a machine, `peer-...`) or an **internal tool** (the program's own: the
janitor, voice transcription, the weekly report, the device rangers). Internal tools carry the `internal` flag (control
migration 19), set by `memcp connect NAME --internal`, by the janitor's key creation in the add-on, or from the page.
They never appear in the list of connected assistants nor in the apps' connection lists: the page keeps them in a
collapsed section, *Internal tools*, with their own controls (revoke, treat as an assistant). Revoked connections sit
in their own collapsed list until removed.

Rotating gives a new key at the same address, same rights and expiry; the old key stops at once. Deleting is only for a
revoked connection and removes it from the list; the audit lines about it stay.

## People

| Op | core | CLI | HTTP | page | MCP |
|---|---|---|---|---|---|
| Create | `users::create_user`, `approve_request` | `connect`, requests on the page | `POST /users`, `POST /requests/approve` | add a person, approve a request | - |
| Read | `users::list_users`, `users::user` | `person list`, `person show NAME` | `GET /users`, `GET /user`, `GET /state` | people list | - |
| Update: display, role | `users::set_display`, `set_admin` | `person set NAME --display D --role admin|member` | `POST /users/update` (self: display only) | edit | - |
| Update: heir | `users::set_heir` | `person heir NAME [HEIR]` | `POST /heir/set` | safety block | - |
| Update: recovery key | `users::make_recovery_key`, `clear_recovery_key` | `person recovery-key NAME [--clear]` | `POST /recovery/make`, `POST /recovery/clear` | safety block | - |
| Disable / enable | `users::set_disabled` | `person disable NAME`, `person enable NAME` | `POST /users/update` | disable / enable | - |
| Delete = disable + export | `people::retire_person` | `person delete NAME --to DIR [--dry-run]` | `POST /users/delete` | "Remove" | - |

Deleting a person never loses a memory: the person is disabled (their keys stop), their whole memory is exported to
`archive/people/` and verified, and the row stays so that the export and the audit still name them. The last
administrator cannot be removed, nor can you remove yourself.

## Notes (facts)

| Op | core | CLI | HTTP | page | MCP |
|---|---|---|---|---|---|
| Create | `facts::remember` | `note add SPACE TEXT [--tag T]` | `POST /fact/add` | "New note", journal | `remember` |
| Read (one, history, list, search) | `facts::read_fact`, `history`, `recall` | `note show SPACE ID`, `note list SPACE` | `GET /fact`, `GET /facts` | note view, journal, notes | `recall`, `read_fact`, `history` |
| Update = supersede | `facts::supersede` | `note edit SPACE ID TEXT` | `POST /fact/supersede` | "Save the correction" | `supersede` |
| Delete = retire with a reason | `facts::retire` | `note retire SPACE ID --reason R` | `POST /fact/retire` | "Remove" | `retire` |
| Restore a retired note (new version) | `facts::restore` | `note restore SPACE ID` | `POST /fact/restore` | "Bring back" on a retired note | `restore` |

A retired note keeps its text and its history. Restoring writes a new active note with the same text and tags that
points back at the retired one; the retired one stays retired in the history.

## Keywords, tags, vocabularies, library folders

| Object | Create | Read | Update | Delete | CLI | HTTP | page |
|---|---|---|---|---|---|---|---|
| Keyword of a space | `rules::edit` | `rules::read` | (add and remove) | `rules::edit` | `rules add-keyword`, `remove-keyword`, `show` | `POST /space/rules` `{add_keywords, remove_keywords}` | chips |
| Tag of a space (a filing tag) | `rules::edit` | `rules::read` | (add and remove) | `rules::edit` | `rules add-tag`, `remove-tag` | `POST /space/rules` `{add_tags, remove_tags}` | chips |
| Vocabulary | `vocabulary::import` | `vocabulary::list` | `vocabulary::import` (same name replaces), `add_terms`, `remove_terms` | `vocabulary::remove` | `vocabulary import|list|remove|term-add|term-remove` | `GET /space/vocabularies`, `POST /space/vocabulary` | vocabularies block |
| Library root | `library::add_root` | `library::roots` | `library::rescan_all` | `library::remove_root` | `library add|scan|list|remove` | `GET /space/libraries`, `POST /space/library` (administrators) | libraries block |

| Folder of a space on this node | `folders::set` | `folders::info`, `folders::get` | `folders::set`, `folders::sync_space` | `folders::set` with no path (the folder is left as it is) | `folder set|get|sync` | `GET /space/folder`, `POST /space/folder` (a path: administrators) | "Folder on this node" block |

`library add` is the read-only alias of the folder of a space ([FOLDERS.md](FOLDERS.md)). Removing a library root forgets its index only; the person's folder is never touched. Mounting a folder by HTTP is for
administrators, because it names a path on the machine.

## What assistants never get

Create or delete a space, a node, a connection or a person; rotate a key; write a guide. Those need the person, on the
page or the command line. An assistant that wants one of them leaves a note.
