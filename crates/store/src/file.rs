use crate::fact::validate_time;
use crate::list::{json_array, Cursor};
use crate::space::require_space;
use crate::{FactStatus, ListQuery, Page, Store, StoreError};
use rusqlite::{named_params, params, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub(crate) const COLUMNS: &str =
    "id, space, name, mime, size, sha256, note, created_at, status, reason";

pub struct NewFile<R> {
    pub space: String,
    pub name: String,
    pub mime: String,
    pub reader: R,
    /// Upper bound in bytes for this upload.
    pub max_size: u64,
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Active,
    Retired,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FileInfo {
    pub id: String,
    pub space: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub sha256: String,
    pub note: Option<String>,
    pub created_at: i64,
    pub status: FileStatus,
    pub reason: Option<String>,
}

impl Store {
    pub fn file_put(&self, new: NewFile<impl Read>) -> Result<FileInfo, StoreError> {
        self.put_file(new, false)
    }

    /// Stores a file once per active space, name, and content digest.
    pub fn file_put_if_absent(&self, new: NewFile<impl Read>) -> Result<FileInfo, StoreError> {
        self.put_file(new, true)
    }

    fn put_file(&self, new: NewFile<impl Read>, if_absent: bool) -> Result<FileInfo, StoreError> {
        {
            let conn = self.reader()?;
            require_space(&conn, &new.space)?;
        }
        validate_name(&new.name)?;
        let limit = new
            .max_size
            .min(self.options.max_file_size)
            .min(i64::MAX as u64);
        // Keep the staging file protected until it is persisted or dropped.
        let staging_lock = self.blob_staging_lock()?;
        staging_lock.lock_shared()?;
        // Serialize reservations across Store handles and processes until commit.
        let _reservation = self.reserve_upload()?;
        let usage = self.file_storage()?;
        let has_room = usage.can_reserve(limit);
        let (temp, size, sha256) = if has_room {
            stage(&self.blobs, new.reader, limit)?
        } else if if_absent {
            // At quota, an exact replay may repair a blob already charged to this account.
            let Some(replay_limit) = self.replay_size_limit(&new.space, &new.name)? else {
                return Err(crate::quota::exceeded());
            };
            let limit = limit.min(replay_limit);
            let mut bytes = Vec::new();
            new.reader.take(limit + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                return Err(crate::quota::exceeded());
            }
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let reader = self.reader()?;
            let Some(file) = existing_file(&reader, &new.space, &new.name, &hash)? else {
                return Err(crate::quota::exceeded());
            };
            if blob_path(&self.blobs, &hash)?.try_exists()? {
                return Ok(file);
            }
            drop(reader);
            stage(&self.blobs, bytes.as_slice(), limit)?
        } else {
            return Err(crate::quota::exceeded());
        };
        let path = blob_path(&self.blobs, &sha256)?;
        let created_at = validate_time(None)?;
        temp.as_file().sync_all()?;
        #[cfg(test)]
        self.durability_events.lock().unwrap().push("blob");
        let shard = path
            .parent()
            .ok_or_else(|| StoreError::Invalid("Invalid blob path".into()))?;
        fs::create_dir_all(shard)?;
        let mut temp = if path.try_exists()? { None } else { Some(temp) };
        let result = self.write(|tx| {
            #[cfg(test)]
            self.durability_events.lock().unwrap().push("transaction");
            if if_absent {
                if let Some(file) = existing_file(tx, &new.space, &new.name, &sha256)? {
                    self.persist_blob(&mut temp, &path, shard)?;
                    return Ok(file);
                }
            }
            if !has_room {
                return Err(crate::quota::exceeded());
            }
            let file = tx.query_row(
                &format!(
                    "INSERT INTO files (id, space, name, mime, size, sha256, note, created_at, status)
                     VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active')
                     RETURNING {COLUMNS}"
                ),
                params![new.space, new.name, new.mime, size, sha256, new.note, created_at],
                file_from_row,
            )?;
            self.persist_blob(&mut temp, &path, shard)?;
            Ok(file)
        });
        #[cfg(test)]
        if result.is_ok() {
            self.durability_events.lock().unwrap().push("commit");
        }
        result
    }

    fn replay_size_limit(&self, space: &str, name: &str) -> Result<Option<u64>, StoreError> {
        Ok(self.reader()?.query_row(
            "SELECT MAX(size) FROM files WHERE space = ?1 AND name = ?2 AND status = 'active'",
            (space, name),
            |row| row.get(0),
        )?)
    }

    fn persist_blob(
        &self,
        temp: &mut Option<NamedTempFile>,
        path: &Path,
        shard: &Path,
    ) -> Result<(), StoreError> {
        if let Some(temp) = temp.take() {
            temp.persist(path).map_err(|err| err.error)?;
        }
        // Also sync an existing path left by an interrupted upload.
        fs::File::open(shard)?.sync_all()?;
        #[cfg(test)]
        self.durability_events.lock().unwrap().push("shard");
        fs::File::open(&self.blobs)?.sync_all()?;
        #[cfg(test)]
        self.durability_events.lock().unwrap().push("blobs");
        Ok(())
    }

    pub fn file_get(&self, id: &str) -> Result<(FileInfo, PathBuf), StoreError> {
        let conn = self.reader()?;
        let file = conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM files WHERE id = ?1 AND status = 'active'"),
                [id],
                file_from_row,
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;
        let path = blob_path(&self.blobs, &file.sha256)?;
        Ok((file, path))
    }

    pub fn file_list(&self, query: ListQuery) -> Result<Page<FileInfo>, StoreError> {
        if !(1..=200).contains(&query.limit) {
            return Err(StoreError::Invalid("List limit must be 1 to 200".into()));
        }
        if !query.tags_any.is_empty() || !query.tags_all.is_empty() {
            return Err(StoreError::Invalid("Files do not have tags".into()));
        }
        let cursor = query.cursor.as_deref().map(Cursor::parse).transpose()?;
        let spaces = json_array(&query.spaces)?;
        let status = query.status.unwrap_or(FactStatus::Active).as_str();
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "WITH boundary(snapshot) AS (
                SELECT coalesce(:snapshot, (SELECT coalesce(max(rowid), 0) FROM files))
             )
             SELECT {COLUMNS}, files.rowid, boundary.snapshot
             FROM files CROSS JOIN boundary
             WHERE status = :status AND files.rowid <= boundary.snapshot
               AND (:rowid IS NULL OR (created_at, files.rowid) < (:at, :rowid))
               AND (:since IS NULL OR created_at >= :since)
               AND (:until IS NULL OR created_at <= :until)
               AND (:spaces = '[]' OR space IN (SELECT value FROM json_each(:spaces)))
             ORDER BY created_at DESC, files.rowid DESC
             LIMIT :limit"
        ))?;
        let mut rows = stmt.query(named_params! {
            ":snapshot": cursor.as_ref().map(|c| c.snapshot),
            ":at": cursor.as_ref().map(|c| c.created_at),
            ":rowid": cursor.as_ref().map(|c| c.rowid),
            ":status": status,
            ":since": query.since,
            ":until": query.until,
            ":spaces": spaces,
            ":limit": query.limit + 1,
        })?;
        let mut items = Vec::new();
        let mut last = None;
        while let Some(row) = rows.next()? {
            if items.len() == query.limit {
                return Ok(Page {
                    items,
                    next_cursor: last.as_ref().map(Cursor::encode),
                });
            }
            let file = file_from_row(row)?;
            last = Some(Cursor {
                snapshot: row.get(11)?,
                created_at: file.created_at,
                rowid: row.get(10)?,
            });
            items.push(file);
        }
        Ok(Page {
            items,
            next_cursor: None,
        })
    }

    pub fn file_retire(&self, id: &str, reason: &str) -> Result<(), StoreError> {
        let sha256 = self.write(|tx| {
            let sha256: String = tx
                .query_row("SELECT sha256 FROM files WHERE id = ?1", [id], |row| row.get(0))
                .optional()?
                .ok_or(StoreError::NotFound)?;
            tx.execute(
                "UPDATE files SET status = 'retired', reason = ?2 WHERE id = ?1 AND status = 'active'",
                (id, reason),
            )?;
            Ok(sha256)
        })?;
        // Commit the retirement before deleting bytes, then recheck under the write lock.
        // A retry can finish cleanup if removing the blob failed.
        let staging_lock = self.blob_staging_lock()?;
        staging_lock.lock()?;
        self.write(|tx| {
            let still_active = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE sha256 = ?1 AND status = 'active')",
                [&sha256],
                |row| row.get::<_, bool>(0),
            )?;
            if !still_active {
                match fs::remove_file(blob_path(&self.blobs, &sha256)?) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(())
        })
    }
}

fn existing_file(
    db: &rusqlite::Connection,
    space: &str,
    name: &str,
    sha256: &str,
) -> Result<Option<FileInfo>, StoreError> {
    Ok(db
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM files WHERE space = ?1 AND name = ?2
                 AND sha256 = ?3 AND status = 'active' ORDER BY rowid LIMIT 1"
            ),
            (space, name, sha256),
            file_from_row,
        )
        .optional()?)
}

pub(crate) fn validate_name(name: &str) -> Result<(), StoreError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(StoreError::Invalid(
            "A file name must be a bare name".into(),
        ));
    }
    Ok(())
}

fn stage(
    blobs: &Path,
    reader: impl Read,
    limit: u64,
) -> Result<(NamedTempFile, u64, String), StoreError> {
    let mut temp = NamedTempFile::new_in(blobs)?;
    let mut hasher = Sha256::new();
    let mut reader = reader.take(limit + 1);
    let mut buf = [0; 16 * 1024];
    let mut size = 0;
    loop {
        let read = match reader.read(&mut buf) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > limit {
            return Err(StoreError::Invalid(format!(
                "File exceeds the {limit}-byte limit"
            )));
        }
        let bytes = &buf[..read];
        hasher.update(bytes);
        temp.write_all(bytes)?;
    }
    Ok((temp, size, format!("{:x}", hasher.finalize())))
}

pub(crate) fn blob_path(blobs: &Path, sha256: &str) -> Result<PathBuf, StoreError> {
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(StoreError::Invalid("Invalid stored SHA-256 digest".into()));
    }
    Ok(blobs.join(&sha256[..2]).join(sha256))
}

pub(crate) fn file_from_row(row: &Row<'_>) -> rusqlite::Result<FileInfo> {
    let status: String = row.get(8)?;
    let status = match status.as_str() {
        "active" => FileStatus::Active,
        "retired" => FileStatus::Retired,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(FileInfo {
        id: row.get(0)?,
        space: row.get(1)?,
        name: row.get(2)?,
        mime: row.get(3)?,
        size: row.get(4)?,
        sha256: row.get(5)?,
        note: row.get(6)?,
        created_at: row.get(7)?,
        status,
        reason: row.get(9)?,
    })
}
