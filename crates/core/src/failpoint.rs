//! Points where a test stops an operation, as a crash there would: the steps before are written, the steps after are
//! not. The test then opens the memory again, runs [`crate::Brain::recover`] (what a node does when it starts) and
//! checks that the operation ended done once or not done (see `docs/CRASH.md`).
//!
//! A point is armed for the current thread only, so tests running side by side do not stop each other; an operation
//! stopped there answers an error, and does nothing it would have done after the point (no cleanup either, as after a
//! `kill -9`). When nothing is armed, a point costs a read of a thread-local cell.
//!
//! Each point is named by a constant of [`at`], `<module>.<operation>.<step>`.

use std::cell::Cell;

use crate::error::{Error, Result};

/// The points, each after a step of an operation.
pub mod at {
    /// `files::put*`: the bytes are on disk, no record is written yet.
    pub const FILE_PUT_BYTES: &str = "file.put.bytes";
    /// `files::put*`: the note, the record and its event are written in the transaction, not committed.
    pub const FILE_PUT_COMMIT: &str = "file.put.commit";
    /// `files::retire`: the record, its note and its event are written in the transaction, not committed.
    pub const FILE_RETIRE_COMMIT: &str = "file.retire.commit";
    /// `files::erase`: the record, its note and its event are written in the transaction, not committed.
    pub const FILE_ERASE_COMMIT: &str = "file.erase.commit";
    /// `files::erase`: committed, the bytes are still on disk.
    pub const FILE_ERASE_BYTES: &str = "file.erase.bytes";
    /// `sync::apply`: the events are committed, the bytes of the recordings they erase are still on disk.
    pub const SYNC_APPLY_BYTES: &str = "sync.apply.bytes";
    /// `sync::prune_window`: the pruning is committed, the bytes of the files it removed are still on disk.
    pub const SYNC_PRUNE_BYTES: &str = "sync.prune.bytes";
    /// A move of notes: the intents are recorded, no copy is written.
    pub const MOVE_JOURNAL: &str = "move.journal";
    /// A move of notes: the copies are written, no source is retired.
    pub const MOVE_COPIES: &str = "move.copies";
    /// A new space: its file is linked in under its name, it is not registered.
    pub const SPACE_CREATE_FILE: &str = "space.create.file";
    /// A deletion: the export is checked and the journal written (`deleting`), nothing else is done.
    pub const ARCHIVE_JOURNAL: &str = "space.archive.journal";
    /// A deletion into another space: the merge is committed, the space is still registered.
    pub const ARCHIVE_MERGED: &str = "space.archive.merged";
    /// A deletion: the space is unregistered with the mark of its archive, its files are where they were.
    pub const ARCHIVE_UNREGISTERED: &str = "space.archive.unregistered";
    /// A deletion: the database is in the archive, its log and its files are not yet.
    pub const ARCHIVE_DB: &str = "space.archive.db";
    /// A restore: the journal says `restoring`, nothing has moved.
    pub const RESTORE_JOURNAL: &str = "space.restore.journal";
    /// A restore: the database is back, its log and its files are not yet.
    pub const RESTORE_DB: &str = "space.restore.db";
    /// A restore: every piece is back, the space is not registered.
    pub const RESTORE_FILES: &str = "space.restore.files";
    /// A restore: registered, the journal not closed.
    pub const RESTORE_REGISTERED: &str = "space.restore.registered";
    /// A layout change: committed in the control database with its mark, not yet in the space's log.
    pub const LAYOUT_CONTROL: &str = "layout.set.control";
    /// A layout change: in the space's log, its mark not yet cleared.
    pub const LAYOUT_LOGGED: &str = "layout.set.logged";
    /// A merge: its journal is written and its new spaces made, no target is committed.
    pub const MERGE_SPACES: &str = "merge.run.spaces";
    /// A merge: the first target is committed, not the others.
    pub const MERGE_TARGET: &str = "merge.run.target";
    /// An adoption of an export: the databases and files are in place, nothing is registered.
    pub const ADOPT_FILES: &str = "export.adopt.files";
}

thread_local! {
    static ARMED: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// For the tests: the operations of this thread stop at `point` (one of [`at`]); `None` disarms.
#[doc(hidden)]
pub fn arm(point: Option<&'static str>) {
    ARMED.with(|a| a.set(point));
}

/// Stops here when the test armed this point.
pub(crate) fn hit(point: &'static str) -> Result<()> {
    if ARMED.with(Cell::get) == Some(point) {
        return Err(Error::Invalid(format!("stopped at {point} (failpoint)")));
    }
    Ok(())
}
