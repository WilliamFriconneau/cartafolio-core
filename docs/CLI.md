# The command line

`memcp` is first a command line: every function of the memory is a command, and every command works two ways.

- **Local**: `memcp --data DIR <command>` opens the data directory itself. It needs to be on the machine and to be allowed
  into the directory.
- **Remote**: `memcp --node ADDRESS --key-file FILE <command>` talks to a node that is **running**, over HTTP, with a key.
  The data stay where the node keeps them. This is the way on a Mac, where the data of the app live in its sandbox and
  no other program can open them, and the way from any other machine.

The commands are the same in both modes, and **so is what they print**: the node runs the very function the local command
line runs and sends back the text. `--json` prints the node's raw JSON answer instead.

## Reaching a node

Every `MEMCP_*` environment variable read by the commands has a `CARTAFOLIO_*` alias. When both spellings are set, the `CARTAFOLIO_*` value wins.

| What | Option | Environment |
|---|---|---|
| The node | `--node http://127.0.0.1:8791` | `MEMCP_NODE` |
| The key, in a file (preferred: it does not show in `ps`) | `--key-file FILE` | `MEMCP_KEY_FILE` |
| The key, as text | `--key KEY` | `MEMCP_KEY` |
| A connection key's name (see below) | `--agent NAME` | `MEMCP_AGENT` |
| The node's raw JSON | `--json` | |
| Force the data directory when `MEMCP_NODE` is set | `--local` | |

`--data` and `--node` together are refused: say which one you mean. The page of the node must be open on the address you
reach (it is, on the loopback; elsewhere the node needs `--console-admin`, see [LINUX.md](LINUX.md)).

### Which key

| Key | Made with | Opens | Used for |
|---|---|---|---|
| **Console key** (`hcpc_...`) | `memcp console-token PERSON` on the node, or the page | the node's own routes, `/admin/api/...` | every command below |
| **Connection key** (`hcp_...`) | `memcp connect`, `memcp open`, the page | the MCP tools of the node, `/mcp/NAME` | with `--agent NAME`: the note commands and `space list` |

A console key is a person's: it acts for that person. A **member** runs what concerns their own spaces, connections, notes
and nodes; a command that touches every person or the node's own machine (`person`, `connections`, `library`, `backup`,
`doctor`, `import`, `merge`, `adopt`...) needs an **administrator's** key; an administrator may act for anyone with
`--user`. A member who names someone else with `--user` is refused. The "Key" column of the table says which.

A key that lacks the rights gets a clear sentence, for instance `not allowed with this key: person list needs an
administrator's key; this key is sam's, a member`, and exit code 4.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | done |
| 1 | the command failed (its message is printed: unknown space, nothing to do, a refusal of the memory) |
| 2 | the command line is wrong (missing word, unknown command, `--data` with `--node`, a command that only works on the machine itself) |
| 3 | the node refused the key: unknown, revoked or expired; or a connection key given without `--agent` |
| 4 | the key lacks the rights for this command |
| 5 | the node cannot be reached (down, wrong address); the address is in the message |
| 6 | the node answered something this program does not expect (a node older than this program, a pending route) |

Locally only 0, 1 and 2 happen (a few commands keep their own code, such as 78 for a missing setting).

## Files and streams

- **Down.** `export --to DIR` asks the node for the memory as a tar (`GET /admin/api/export`). The node writes it to a
  temporary file, block by block, and sends it from there; this program unpacks it as it arrives. Neither side holds the
  memory whole. Then it checks the fingerprints and prints the same report as the local command. A key exports **its
  own person's** memory (`--user` is refused).
- **Up.** The commands that read files of your machine (`adopt DIR`, `merge --plan DIR`, `merge --apply FILE`,
  `import --folder DIR --rules FILE`, `vocabulary import FILE`, `intake send FILE...`) first send them to the node with
  `POST /admin/api/upload`, in chunks of 8 MiB (`MEMCP_UPLOAD_CHUNK` changes it, in bytes), one file after the other, then
  run the command on what was sent. A chunk is written at the offset it names and never twice; the node refuses names that
  leave the folder, keeps at most 8 GiB per upload, and removes the folder when the command is over (a folder sent and
  never used is removed after 12 hours). Only files a command named and this program sent are ever read for a remote
  caller. One exception: the plan of `merge --plan EXPORT` names the copy of the export that the node made, so that copy
  is kept (for 12 hours) for `merge --apply PLAN`; plan again after that. What a command prints about a file it was
  given says its name on your machine.
- **On the node's machine.** An option that names a place the command *writes* (`backup --to DIR`, `merge --backup-to DIR`)
  or a folder the node *mounts* (`library add PATH`) is a path **on the node's machine**, and only an administrator's key
  may use it. `merge --plan EXPORT --out FILE` writes `FILE` on your machine.

## Commands

The table is generated from the command list in `crates/server/src/cli_table.rs`; a test fails when a command of the usage
text (`memcp --help`) is missing from it, when a command has no remote form and no reason, or when this table is out of
date (`MEMCP_BLESS=1 cargo test -p memcp-server --test cli_parity` rewrites it).

<!-- table:start -->
| Command | What it does | Local (`--data`) | Remote (`--node`) | Key |
|---|---|---|---|---|
| `init` | creates the data directory, the control database and the spaces | yes | no: creates the data directory of this machine | - |
| `setup` | first run: data directory, first memory, memcp.toml | yes | no: prepares this machine | - |
| `config` | checks, shows or prints the settings file | yes | no: reads the settings file of this machine | - |
| `prepare` | what the janitor's service unit runs first | yes | no: writes files on this machine | - |
| `serve` | runs the node | yes | no: it is the node | - |
| `mcp` | MCP on standard input and output | yes | no: standard input and output of a program on this machine; a remote assistant uses the node's MCP address | - |
| `skills-export` | writes the professions as Claude skills (a plugin marketplace) | yes | no: writes a folder of this machine from the built-in professions | - |
| `purge` | removes notes for good, by id, on this machine only | yes | no: deletes rows of the database of this machine, and must never be asked of a node by a key | - |
| `repair provenance` | puts back the date, author, source and origin a retag lost | yes | no: edits rows of the database of this machine, after a count | - |
| `tags remove` | takes tags off the notes that carry them | yes | no: edits rows of the database of this machine, after a count | - |
| `professions` | lists the professions the person added and the proposals | yes | no: reads the professions file in the data folder of this machine | - |
| `professions accept` | accepts an assistant's proposed profession | yes | no: moves a proposal inside the data folder of this machine | - |
| `professions reject` | drops an assistant's proposed profession | yes | no: edits the proposals file in the data folder of this machine | - |
| `verify-export` | checks an export folder | yes | no: checks a folder of this machine and needs no node | - |
| `transcribe` | one pass over the recordings waiting | yes | no: the node does it by itself while it serves, from its own settings | - |
| `watch` | looks at the folders of `watch` | yes | no: watches folders of this machine | - |
| `intake send` | puts files into a space's inbox | yes | yes, files are sent first | any person |
| `intake text` | puts a note into the inbox | yes | yes, runs on the node | any person |
| `intake absorb` | one pass over the folders of `absorb` | yes | no: reads folders named in the settings of the node's machine | - |
| `intake status` | recordings waiting and folders watched | yes | no: reads the settings file of the node's machine | - |
| `intake discover` | nodes found on this network | yes | no: looks at the network of this machine | - |
| `catalogue` | lists the kinds of space | yes | yes, runs on the node | any person |
| `space make` | creates a person's space of a kind | yes | yes, runs on the node | any person |
| `vault add` | former name of `space make` | yes | yes, runs on the node | any person |
| `space add` | a space named by hand | yes | yes, runs on the node | administrator |
| `space list` | lists the spaces of the node | yes | yes, runs on the node | administrator |
| `space kind` | gives a space a kind | yes | yes, runs on the node | administrator |
| `space new` | creates a space with a label | yes | yes, runs on the node | any person |
| `space show` | one space: label, kind, counts, rules | yes | yes, runs on the node | any person |
| `space tree` | the spaces as a tree | yes | yes, runs on the node | any person |
| `space set` | label, parent, position, kind | yes | yes, runs on the node | any person |
| `space rename` | renames a space on disk and in the control database | yes | no: moves the database and files of this machine and rewrites its control database | - |
| `space delete` | moves a space's notes into another, or archives it | yes | yes, runs on the node | any person |
| `space archived` | lists the archived spaces | yes | yes, runs on the node | any person |
| `space restore` | brings an archived space back | yes | yes, runs on the node | any person |
| `folder get` | the folder of a space | needs a node | yes, through the page's routes | any person |
| `folder sync` | one pass of the folder of a space now | needs a node | yes, through the page's routes | any person |
| `folder set` | sets or clears the folder of a space | needs a node | yes, through the page's routes | any person |
| `rules` | keywords and tags of a space | yes | yes, runs on the node | any person |
| `vocabulary import` | imports a list of terms into a space | yes | yes, files are sent first | administrator |
| `vocabulary list` | lists the vocabularies | yes | yes, runs on the node | administrator |
| `vocabulary remove` | removes a vocabulary | yes | yes, runs on the node | administrator |
| `vocabulary term-add` | adds terms to a vocabulary | yes | yes, runs on the node | administrator |
| `vocabulary term-remove` | removes terms from a vocabulary | yes | yes, runs on the node | administrator |
| `library add` | mounts a folder of the node's machine on a space | yes | yes, runs on the node | administrator |
| `library scan` | reads the mounted folders again | yes | yes, runs on the node | administrator |
| `library list` | lists the mounted folders | yes | yes, runs on the node | administrator |
| `library remove` | unmounts a folder | yes | yes, runs on the node | administrator |
| `node` | the other nodes: list, rename, note, pause, resume, revoke, forget, and the sync policy per space | yes | yes, runs on the node | any person |
| `connection` | show, rename, rotate, delete a connection; also list, new, regrant, revoke, internal | yes | yes, runs on the node | any person |
| `connections` | lists the connections | yes | yes, runs on the node | administrator |
| `connect` | creates a connection and shows its key once | yes | yes, runs on the node | any person |
| `open` | opens one space to an assistant | yes | yes, runs on the node | any person |
| `regrant` | changes the rights of a connection | yes | yes, runs on the node | administrator |
| `revoke` | cuts a connection | yes | yes, runs on the node | administrator |
| `internal` | marks a connection as the program's own | yes | yes, runs on the node | administrator |
| `readonly` | forces a connection read-only | yes | yes, runs on the node | administrator |
| `person-guide` | tells or not the person's guide to a connection | yes | yes, runs on the node | administrator |
| `limit` | sets a limit of a connection | yes | yes, runs on the node | administrator |
| `person` | list, show, new, set, heir, recovery-key, disable, enable, delete | yes | yes, runs on the node | administrator |
| `note` | add, show, list, recall, tags, edit, retire, restore | yes | yes, runs on the node | any person |
| `console-token` | a token that opens the page | yes | yes, runs on the node | administrator |
| `invite code` | a code (and link) a device exchanges for a key to the inbox | needs a node | yes, through the page's routes | any person |
| `invite ticket` | a link with a one-time ticket another node joins with | needs a node | yes, through the page's routes | any person |
| `invite pending` | devices asking to join | needs a node | yes, through the page's routes | any person |
| `invite approve` | lets a device in | needs a node | yes, through the page's routes | any person |
| `invite deny` | turns a device away | needs a node | yes, through the page's routes | any person |
| `invite allow` | lets a device come back without the words, with a one-time ticket | needs a node | yes, through the page's routes | any person |
| `join` | asks another node (link, id or address) to let this node in | needs a node | yes, through the page's routes | administrator |
| `export` | writes a person's memory to a folder | yes | yes, streamed down | any person |
| `adopt` | takes the spaces of an export into the node | yes | yes, files are sent first | administrator |
| `merge` | plans or applies the merge of another memory | yes | yes, files are sent first | administrator |
| `import` | indexes a Markdown folder | yes | yes, files are sent first | administrator |
| `duplicates` | what says the same thing twice | yes | yes, runs on the node | administrator |
| `backup` | a consistent copy of every database | yes | yes, runs on the node | administrator |
| `doctor` | reads the installation and says what deserves a look | yes | yes, runs on the node | administrator |
| `status` | what this memory holds | yes | yes, runs on the node | any person |

<!-- table:end -->

A command that is **local only** says so, with the reason, when it is given `--node` (exit 2). Everything else has a remote
form. The commands marked "needs a node" (`invite`, `join`, `folder`) are the other way round: they read what a *running*
node holds (the requests waiting, the tickets, the folders of its spaces), so they only exist against a node, and say so
(exit 2) when given a directory.

### Notes, with a connection key

With `--agent NAME` and the key of that connection, these commands use the assistants' tools (`remember`, `recall`,
`history`, `supersede`, `retire`, `restore`, `tags_set`, `spaces`) and so only reach what the connection reaches:

```
memcp --node http://127.0.0.1:8791 --key-file helper.key --agent helper note add work "The boiler needs a service in May."
memcp --node http://127.0.0.1:8791 --key-file helper.key --agent helper note recall work boiler
```

Any other command with a connection key answers exit 4: it needs a console key.

## Examples

```sh
# A key for the command line, made on the node (once), kept in a file only you read
memcp --data ~/memcp console-token alex | grep -o 'hcpc_[A-Za-z0-9_]*' > ~/.config/memcp/console.key
chmod 600 ~/.config/memcp/console.key
export MEMCP_NODE=http://127.0.0.1:8791 MEMCP_KEY_FILE=~/.config/memcp/console.key

memcp status
memcp space new "Cars" --parent work --keywords garage,tyres
memcp space delete cars --into work --dry-run
memcp note add work "The garage is open on Mondays." --tag cars
memcp note recall work garage --limit 5
memcp note tags work f_abc123 --add cars
memcp person new sam --display Sam --role member          # an administrator's key
memcp connect helper --grant work:rw                       # prints the connection's key once
memcp export --to ~/backup/memory                          # streamed down to this machine
memcp adopt ~/backup/memory/memcp-export-alex-... --prefix old   # sent up in chunks, then adopted
memcp merge --plan ~/other-export --user alex --out plan.txt     # plan.txt is written here
memcp merge --apply plan.txt
memcp invite code                                          # a code and a link for a phone
memcp invite ticket                                        # a link with a one-time ticket, for another node
memcp invite pending                                       # devices waiting for a yes
memcp node policy peer-laptop-ab12 health off             # this node keeps health apart from that machine (both, send, receive, off)
memcp node policy peer-laptop-ab12 --later receive         # spaces made later: taken from it, not sent
memcp invite approve REQUEST_ID
memcp join 'memcp://node?id=...&t=...'                      # this node asks another to let it in (or ID, or ADDRESS)
memcp --json status                                        # {"code":0,"stdout":"...","stderr":""}
memcp --local --data ~/memcp status                        # the directory, whatever MEMCP_NODE says
```

## The routes behind it

All under `/admin/api` and authorised like the page's: `Authorization: Bearer <console key>`, and for a POST the header
`X-Requested-With: memcp`. Each is audited (`remote_command` in the control audit log, plus the audit of the function it
runs). See [CHANGELOG-API.md](CHANGELOG-API.md).

| Route | What |
|---|---|
| `POST /cli` `{argv: [...]}` | runs a command for the key's person; answers `{code, stdout, stderr}` |
| `POST /upload?id=&path=&offset=` | one chunk of one file of a folder sent for a command (raw bytes) |
| `POST /upload/discard` `{id}` | forgets a folder that was sent |
| `GET /export` | the person's memory as a tar, streamed |
| `POST /nodes/invite` | a link with a one-time ticket, for another node of the person |
| `POST /nodes/add` `{link}`, `GET /join-memory?id=` | asks another node (link, id or address) to let this one in, and follows it |
| `GET /join/pending`, `POST /join/approve`, `/join/deny`, `/join/allow-device`, `POST /pairing` | the devices joining |
| `GET /space/folder?space=`, `POST /space/folder` `{space, folder}` | the folder of a space (**pending**: with the space folders; the command says so on a 404) |

The page's own routes for spaces, nodes, connections, people, vocabularies and library folders (see [CRUD.md](CRUD.md))
are unchanged and reachable by a console key too; `POST /cli` is the one door through which the command line reaches every
command with the same text as the local mode.
