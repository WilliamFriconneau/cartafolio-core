//! What a node does when it starts, before it serves anything: it finishes or undoes what a crash, a `kill -9` or a
//! power loss cut off, so that every operation ends done once or not done (see `docs/CRASH.md`).
//!
//! [`Brain::recover`] is idempotent and fast on a big store: it reads each space's records once and lists its folders
//! of bytes once.
//!
//! - Moves of notes cut off are finished ([`Brain::finish_moves`]).
//! - A file whose record has no event in the peers' log (written by an earlier version, which logged it apart) gets
//!   it, under the id derived from the file (`f:<id>`, `fr:<id>`): written twice, it is one event.
//! - The bytes that no record holds any more (a put cut off before its record, a recording erased or a file pruned
//!   before its bytes went) are removed, and so are the files a write left half-done (`.in-*.tmp`, `.<sha>.tmp`,
//!   `.<sha>.assembling`) and the chunks kept aside for a file whose bytes are whole or erased.
//!
//! # The lock of the bytes
//!
//! Several processes may use one data directory (the server and a command, the stdio server of an assistant). The
//! bytes on disk are therefore swept only when no one is writing some: whoever writes bytes that a record will name
//! holds `files.lock` shared, from the first byte to the commit of the record ([`writing`]); the sweep takes it alone,
//! and skips its turn when it cannot ([`Recovered::swept`] false). Removing the bytes of one file also takes it alone
//! ([`drop_unless_held`]), and checks once it holds it that no record took them meanwhile.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::Path;

use rusqlite::Connection;

use crate::brain::Brain;
use crate::error::Result;
use crate::fsx::PrivateFile as _;
use crate::space::Space;

/// What [`Brain::recover`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recovered {
    /// Operations on whole spaces cut off, finished or undone: archives and restores, merges, adoptions of an export,
    /// layout changes not yet in a space's log.
    pub spaces: usize,
    /// Files of a space a creation cut off left (registered nowhere, holding no note), removed.
    pub orphans: usize,
    /// Files of a space registered nowhere that hold notes: left where they are, for the person to decide.
    pub stray: Vec<String>,
    /// What could not be finished, and why (tried again at the next start).
    pub unsettled: Vec<String>,
    /// Notes of a move cut off, finished.
    pub moves: usize,
    /// Events of files missing from the peers' log, written.
    pub events: usize,
    /// Bytes no record holds, removed.
    pub blobs: usize,
    /// Files a write left half-done, removed.
    pub temporary: usize,
    /// Folders of chunks kept aside for nothing, removed.
    pub staged: usize,
    /// Whether the folders of bytes were swept (not when another process was writing bytes).
    pub swept: bool,
}

impl Recovered {
    /// Whether nothing needed doing (the sweep aside).
    pub fn is_clean(&self) -> bool {
        self.moves
            + self.events
            + self.blobs
            + self.temporary
            + self.staged
            + self.spaces
            + self.orphans
            == 0
            && self.stray.is_empty()
            && self.unsettled.is_empty()
    }

    /// One line for the log of the node.
    pub fn summary(&self) -> String {
        let mut extra = String::new();
        if self.spaces + self.orphans > 0 {
            extra.push_str(&format!(
                "; {} operation(s) on spaces finished, {} empty space file(s) left by a creation removed",
                self.spaces, self.orphans
            ));
        }
        if !self.stray.is_empty() {
            extra.push_str(&format!(
                "; space files registered nowhere, holding notes, left in place: {}",
                self.stray.join(", ")
            ));
        }
        if !self.unsettled.is_empty() {
            extra.push_str(&format!(
                "; not finished yet: {}",
                self.unsettled.join("; ")
            ));
        }
        format!(
            "recovery: {} note(s) of a move finished, {} file event(s) logged, {} unheld file(s), {} half-written file(s) and {} chunk folder(s) removed{}{extra}",
            self.moves,
            self.events,
            self.blobs,
            self.temporary,
            self.staged,
            if self.swept {
                ""
            } else {
                " (bytes not swept: another process is writing)"
            }
        )
    }
}

const LOCK: &str = "files.lock";

fn lock_file(brain: &Brain) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .private_file()
        .open(brain.dir().join(LOCK))?)
}

/// Held while bytes are written that a record will name; released when dropped.
pub(crate) struct Writing(#[allow(dead_code)] File);

/// Takes the lock of the bytes shared: from the first byte written to the commit of the record that names them.
pub(crate) fn writing(brain: &Brain) -> Result<Writing> {
    let f = lock_file(brain)?;
    f.lock_shared()?;
    Ok(Writing(f))
}

const SPACES_LOCK: &str = "spaces.lock";

/// Held while an operation on whole spaces runs (a deletion, a restore, a merge, an adoption); released when dropped.
pub(crate) struct Busy(#[allow(dead_code)] File);

fn spaces_lock_file(brain: &Brain) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .private_file()
        .open(brain.dir().join(SPACES_LOCK))?)
}

/// Takes the lock of the operations on whole spaces shared: the start-up pass finishes the ones a crash cut off only
/// when none is under way in another process (it takes it alone).
pub(crate) fn busy(brain: &Brain) -> Result<Busy> {
    let f = spaces_lock_file(brain)?;
    f.lock_shared()?;
    Ok(Busy(f))
}

/// Whether a record of the space still needs the bytes `sha`: one that is not an erased recording.
pub(crate) fn held(conn: &Connection, sha: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT count(*) > 0 FROM files WHERE sha256 = ?1
         AND NOT (status = 'retired' AND retired_reason LIKE 'erased: %')",
        [sha],
        |r| r.get(0),
    )?)
}

/// Removes the bytes `sha` of `space` (and the chunks of it kept aside) unless a record still holds them, after the
/// change that let them go is committed. The caller holds no [`Writing`]. Answers whether they went.
pub(crate) fn drop_unless_held(
    brain: &Brain,
    space: &Space,
    conn: &Connection,
    sha: &str,
) -> Result<bool> {
    let f = lock_file(brain)?;
    f.lock()?;
    if held(conn, sha)? {
        return Ok(false);
    }
    crate::files::remove_blob(brain, space, sha)?;
    Ok(true)
}

/// Whether a space file holds a note or a file: what a person wrote, not the settings a creation writes with it (its
/// rules note).
pub(crate) fn holds_data(conn: &Connection) -> Result<bool> {
    let table = |t: &str| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [t],
            |r| r.get(0),
        )?)
    };
    if table("files")? {
        let n: i64 = conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0))?;
        if n > 0 {
            return Ok(true);
        }
    }
    if table("facts")? {
        let settings = if table("fact_tags")? {
            "SELECT count(*) FROM facts WHERE id NOT IN (SELECT fact_id FROM fact_tags WHERE tag = 'rules')"
        } else {
            "SELECT count(*) FROM facts"
        };
        let n: i64 = conn.query_row(settings, [], |r| r.get(0))?;
        if n > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether an archive, a restore or an adoption of `space` is under way: its files are not to be touched but by it.
pub(crate) fn in_flight(brain: &Brain, space: &Space) -> Result<bool> {
    Ok(brain.archive_in_flight(space) || crate::export::adopting(brain, space)?)
}

/// Removes the file of `space` that a creation cut off left: registered nowhere, nothing under way, and no note or
/// file in it. Done only when no one else is writing (the lock of the bytes alone). Answers whether the name is free.
pub(crate) fn clear_orphan(brain: &Brain, space: &Space) -> Result<bool> {
    let path = brain.dir().join(space.file_name());
    if !path.exists() {
        return Ok(true);
    }
    if brain.has_space(space)? || in_flight(brain, space)? {
        return Ok(false);
    }
    let f = lock_file(brain)?;
    match f.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(false),
        Err(TryLockError::Error(e)) => return Err(e.into()),
    }
    remove_orphan(brain, space)
}

/// [`clear_orphan`] once the lock is held alone.
fn remove_orphan(brain: &Brain, space: &Space) -> Result<bool> {
    let path = brain.dir().join(space.file_name());
    if brain.has_space(space)? || in_flight(brain, space)? {
        return Ok(false);
    }
    let data = {
        let conn = crate::brain::open_db(&path)?;
        holds_data(&conn)?
    };
    if data {
        return Ok(false);
    }
    crate::brain::remove_db_files(&path);
    // Its folder of files, if it is empty.
    let _ = fs::remove_dir(brain.dir().join("files").join(space.as_str()));
    crate::fsx::sync_dir(brain.dir())?;
    Ok(true)
}

/// The data folder swept (the caller holds the lock alone): the files new spaces and adoptions were written under, and
/// the copies a merge reads from, an hour old; then the space files registered nowhere.
fn sweep_data(brain: &Brain, out: &mut Recovered) -> Result<()> {
    let registered: HashSet<String> = brain.spaces()?.iter().map(|s| s.file_name()).collect();
    let Ok(entries) = fs::read_dir(brain.dir()) else {
        return Ok(());
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = || {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|d| d.as_secs() > 3600)
        };
        if name.starts_with(crate::brain::NEW_PREFIX)
            || name.starts_with(crate::export::ADOPT_PREFIX)
            || (name.starts_with(".merge-") && old())
        {
            out.temporary += usize::from(remove(&e.path())?);
            continue;
        }
        let Some(stem) = name.strip_suffix(".sqlite") else {
            continue;
        };
        if registered.contains(&name) {
            continue;
        }
        let Ok(space) = Space::new(stem) else {
            continue;
        };
        if remove_orphan(brain, &space)? {
            out.orphans += 1;
        } else if !in_flight(brain, &space)? && !brain.has_space(&space)? {
            out.stray.push(name);
        }
    }
    Ok(())
}

fn is_half_written(name: &str) -> bool {
    name.starts_with('.') && (name.ends_with(".tmp") || name.ends_with(".assembling"))
}

fn remove(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn strings(conn: &Connection, sql: &str) -> Result<HashSet<String>> {
    Ok(conn
        .prepare(sql)?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// The folders of bytes and of chunks of one space, swept (the caller holds the lock alone).
fn sweep(brain: &Brain, space: &Space, conn: &Connection, out: &mut Recovered) -> Result<()> {
    let needed = strings(
        conn,
        "SELECT DISTINCT sha256 FROM files
         WHERE NOT (status = 'retired' AND retired_reason LIKE 'erased: %')",
    )?;
    let known = strings(conn, "SELECT DISTINCT sha256 FROM files")?;
    let dir = brain.dir().join("files").join(space.as_str());
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_half_written(&name) {
                out.temporary += usize::from(remove(&e.path())?);
            } else if crate::chunks::is_digest(&name) && !needed.contains(&name) {
                out.blobs += usize::from(remove(&e.path())?);
            }
        }
    }
    let staging = brain.dir().join("sync-chunks").join(space.as_str());
    if let Ok(entries) = fs::read_dir(&staging) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !crate::chunks::is_digest(&name) {
                continue;
            }
            // Whole already, or erased: the chunks aside serve nothing. A file no record names yet is being received.
            let whole = dir.join(&name).exists();
            let erased = known.contains(&name) && !needed.contains(&name);
            if whole || erased {
                crate::chunks::drop_staged(brain, space, &name);
                out.staged += 1;
                continue;
            }
            if let Ok(inner) = fs::read_dir(e.path()) {
                for c in inner.flatten() {
                    if is_half_written(&c.file_name().to_string_lossy()) {
                        out.temporary += usize::from(remove(&c.path())?);
                    }
                }
            }
        }
    }
    Ok(())
}

impl Brain {
    /// Finishes or undoes, in every space, what a crash cut off (see the module documentation). Run when the node
    /// starts, before it serves; safe at any time and from several processes.
    pub fn recover(&self) -> Result<Recovered> {
        let mut out = Recovered::default();
        // Whole spaces first: what the other steps read must be registered and in place. Only when no such operation
        // runs in another process: what it is doing is not cut off.
        let alone = spaces_lock_file(self)?;
        match alone.try_lock() {
            Ok(()) => {
                match crate::export::settle_adoptions(self) {
                    Ok(n) => out.spaces += n,
                    Err(e) => out.unsettled.push(format!("adoption: {e}")),
                }
                out.spaces += crate::merge::settle(self, &mut out.unsettled);
                let (n, failed) = self.settle_archives();
                out.spaces += n;
                out.unsettled.extend(failed);
                drop(alone);
            }
            Err(TryLockError::WouldBlock) => out.unsettled.push(
                "an operation on spaces runs in another process: what a crash cut off is settled at the next start"
                    .into(),
            ),
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
        match crate::layout::settle(self) {
            Ok(n) => out.spaces += n,
            Err(e) => out.unsettled.push(format!("layout: {e}")),
        }
        out.moves = self.finish_moves()?;
        let spaces = self.spaces()?;
        for space in &spaces {
            let conn = self.open_space(space)?;
            out.events += crate::sync::log_unlogged_files(&conn)?;
        }
        let f = lock_file(self)?;
        match f.try_lock() {
            Ok(()) => {
                out.swept = true;
                sweep_data(self, &mut out)?;
                for space in &spaces {
                    let conn = self.open_space(space)?;
                    sweep(self, space, &conn, &mut out)?;
                }
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Language;

    #[test]
    fn the_bytes_are_not_swept_while_another_writer_holds_the_lock() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("memcp-recover-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let brain = Brain::init(&dir, &[Space::new("notes")?], Language::English)?;
        let blobs = dir.join("files").join("notes");
        fs::create_dir_all(&blobs)?;
        fs::write(blobs.join(".in-writing.tmp"), b"being copied")?;
        let w = writing(&brain)?;
        let r = brain.recover()?;
        assert!(!r.swept);
        assert!(
            blobs.join(".in-writing.tmp").exists(),
            "a copy under way stays"
        );
        drop(w);
        let r = brain.recover()?;
        assert!(r.swept && r.temporary == 1, "{r:?}");
        drop(brain);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }
}
