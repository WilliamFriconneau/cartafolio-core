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
        self.moves + self.events + self.blobs + self.temporary + self.staged == 0
    }

    /// One line for the log of the node.
    pub fn summary(&self) -> String {
        format!(
            "recovery: {} note(s) of a move finished, {} file event(s) logged, {} unheld file(s), {} half-written file(s) and {} chunk folder(s) removed{}",
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
        let mut out = Recovered {
            moves: self.finish_moves()?,
            ..Recovered::default()
        };
        let spaces = self.spaces()?;
        for space in &spaces {
            let conn = self.open_space(space)?;
            out.events += crate::sync::log_unlogged_files(&conn)?;
        }
        let f = lock_file(self)?;
        match f.try_lock() {
            Ok(()) => {
                out.swept = true;
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
