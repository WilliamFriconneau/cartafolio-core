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
