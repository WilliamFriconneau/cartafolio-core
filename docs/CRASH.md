# Crash safety

The rule: **every operation is guaranteed, even after a `kill -9` or a power loss at any instant.** When the node starts
again, each operation has ended **done once** or **not done**: never half-done, lost or doubled, and the peers' log
(`sync_log`) says exactly what the data holds.

## How

- **One transaction.** What must change together in one space (a file's record, its describing note, their events in
  the peers' log) is written in one SQLite transaction.
- **An intent journal.** A transaction cannot span two SQLite files. An operation over two spaces first records its
  intent in the source (`move_pending`), then does the steps; the journal is cleared in the last step's transaction.
- **Derived ids.** What a resumed step writes has an id that the operation gives (`filed_id` for a moved note, `f:<id>`,
  `fr:<id>` for a file's events), so a step done twice is one row.
- **Bytes after the commit.** Bytes a record will name are written (with `fsync`, then the folder synced after the
  rename) before the record commits; bytes a record lets go are removed after it commits. A crash in between leaves
  bytes no record needs, which the start-up pass removes.
- **The start-up pass** (`Brain::recover`, `crates/core/src/recover.rs`), run before anything is served by every
  long-running program that opens a memory: the server (HTTP and stdio), the manager (the registry and each person's
  memory, when it is first opened), `memcp-sync`, and the apps' own node (`capture-ffi`): it finishes the moves cut off, writes the file events an earlier version left out, and sweeps the folders of
  bytes (bytes no record needs, `.in-*.tmp`, `.<sha>.tmp`, `.<sha>.assembling`, chunks kept aside for a file whose bytes
  are whole or erased). It is idempotent and reads each space once.
- **The lock of the bytes** (`files.lock` in the data directory): a writer of bytes holds it shared until its record
  commits; the sweep holds it alone and skips its turn when another process is writing; removing one file's bytes
  holds it alone and checks again that no record took them. A space being made holds it too, from its file to its
  registration.
- **A file made aside, linked in.** A new space's database is written under `.new-<space>-<random>.sqlite` (migrated,
  its meta, its rules and the events of its layout), closed (its log folded in), made durable, then linked under its
  name with a hard link, which never replaces a file there; one control transaction then registers it with its kind,
  label, place, time and grants. A crash between leaves a file registered nowhere that holds no note: the next
  creation of the name or the start-up pass removes it. A file registered nowhere that holds notes is never removed:
  the pass reports it (`Recovered::stray`). A registered space whose file is gone, with nothing under way, gets an
  empty one. Blobs, an adopted export's databases, archive journals: always a temporary file, `fsync`, rename.
- **Journals of whole spaces.**
  - *Archive* (deleting a space): `archive.json` in its archive folder, written whole (temporary file, `fsync`,
    rename) right after the export is checked, with `state: deleting` and all that is left to do (the target, the
    children, the export). Then: the merge into the target (merging twice changes nothing), the children moved up,
    one control transaction that unregisters the space **and** keeps the mark of its archive for the peers
    (`swarm_archived:`), the database (checkpointed first), its `-wal`, `-shm` and files moved into `live/` (each
    linked then unlinked, never over another file, a piece already moved skipped), `state: archived`. The start-up
    pass goes on from wherever it stopped. A target gone meanwhile: the deletion is given up (`aborted.json`).
  - *Restore*: `state: restoring`, the pieces moved back (never over a file), one control transaction (rows, grants,
    the mark that it is back), `archive.json` renamed `restored.json`. Finished at start. No space is made under a
    name whose archive or restore is under way.
  - An archive an earlier version left half-done (files moved, control rows still there) is undone at start: the
    files come back, the archive is given up; files a restore of an earlier version had begun to move back return to
    the archive.
  - *Merge*: `merge_pending:<id>` in the control database (the plan and the spaces it creates), written before
    anything changes; one commit per target; the journal goes after the last. The start-up pass applies the plan
    again: what is merged already is not merged twice, a space it made counts as made by it.
  - *Adoption of an export*: `adopt_pending:<id>` (the names) before any file; the databases and files placed; one
    control transaction registers them all with their guides and clears the journal. At start, the files of a
    journal's names that nobody registered are removed.
  - *Layout*: `layout_pending:<space>` (a token, the fields) in the control transaction of the change; the space's log
    records the fields and the token in one transaction; the mark goes after. At start, a mark is logged once (the
    token says whether it already is), with the values the control database holds.
- **The lock of whole spaces** (`spaces.lock`): a deletion, a restore, a merge or an adoption holds it shared; the
  start-up pass finishes their journals only when it holds it alone (else it says so and leaves them to the next
  start): it never finishes an operation another process is still doing.
- **Bytes are checked by their size.** A blob under its final name of another size than its record says (torn by an
  earlier version's copy) is never taken for the file: `store_blob`, `hold_or_link`, the chunk assembly and a file
  put write it again (and `store_blob` compares the content when the size matches).

## Operations covered

| Operation | Mechanism |
|---|---|
| `remember`, `supersede`, `retire`, `restore` of a note | one transaction with its event |
| `file_put` (`files::put*`, a watched folder, `intake send`) | bytes, then one transaction: note, record, `f:` event; orphan bytes swept |
| `file_retire` | one transaction: record, note, `fr:` event |
| `file_erase` (a transcribed recording) | one transaction: record, note, `fr:` event marked erased; bytes removed after, or swept; a retry answers done |
| Receiving events from a peer (`sync::apply`) | one transaction; erased bytes removed after, or swept |
| Pruning a window replica | one transaction; bytes removed after, or swept |
| Files left out for a peer without chunks | the mark is cleared only once the peer's cursor is durably before it |
| `notes_move` | intent journal, derived ids, finished at start; a note under a move is not replaced or retired meanwhile |
| A big file received in chunks | chunks and the assembled file written with `fsync` and renamed; leftovers swept |
| Making a space (`space_create`, a vault, the Inbox, a custom space, from a peer's offer, a merge's new space) | file made aside and linked in; one control transaction with kind, label, place, time, grants; orphan file removed |
| Deleting a space (archive, or into another) | archive journal, the mark in the unregistering transaction, pieces moved once; finished at start |
| Restoring an archived space | archive journal (`restoring`), pieces moved back once, one control transaction; finished at start |
| A peer's archive or offer (`swarm`) | as deleting a space; a space made from an offer keeps the peer's time from the start |
| Changing a space's layout | mark in the control transaction, logged once with its token; finished at start |
| Merging an export (`merge::apply`) | merge journal; finished at start, doubling nothing |
| Adopting an export | adoption journal; undone at start |
| A note replaced by a peer while its move was cut off | the move takes the newest version; its copy replaces the older copy in the target; a note retired meanwhile has its copy retired too |
| Forgetting a peer | one transaction |

## Testing it

`crate::failpoint` names a point after each step (`failpoint::at`); a test arms one for its thread
(`failpoint::arm`), the operation stops there as a crash would, the test opens the node again, runs `Brain::recover`
and checks the invariant. `crates/core/tests/crash.rs` does it for each step of the operations above, with a two-node
erasure, and kills (`SIGKILL`) a child process putting big files; `crates/core/tests/moves.rs` does it for moves;
`crates/core/tests/crash_spaces.rs` does it for the life of a space (each step of making, archiving, deleting into
another space, restoring, layout, merge, adoption; two nodes for an archive and a peer's offer; a peer's correction of
a note under a move) and kills a child process archiving and restoring a space with notes and a file.
