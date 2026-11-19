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
  holds it alone and checks again that no record took them.

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

## Testing it

`crate::failpoint` names a point after each step (`failpoint::at`); a test arms one for its thread
(`failpoint::arm`), the operation stops there as a crash would, the test opens the node again, runs `Brain::recover`
and checks the invariant. `crates/core/tests/crash.rs` does it for each step of the operations above, with a two-node
erasure, and kills (`SIGKILL`) a child process putting big files; `crates/core/tests/moves.rs` does it for moves.
