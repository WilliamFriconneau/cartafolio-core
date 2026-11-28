#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(test)]
mod durability_tests;
#[cfg(test)]
mod open_tests;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod search_tests;

mod capture;
mod cursor;
mod error;
mod export;
mod fact;
mod file;
mod list;
mod pin;
mod quota;
mod review_migration;
mod schema;
mod search;
mod space;
mod sweep;
mod tags;
mod todo;

pub use error::StoreError;
pub use fact::{Fact, FactStatus, NewFact, Outcome, Remembered};
pub use file::{FileInfo, FileStatus, NewFile};
pub use list::{ListQuery, OriginAliases, Page};
pub use quota::{FileQuota, FileStorage};
pub use search::{Hit, RecallQuery};
pub use space::SpaceInfo;
pub use sweep::BlobSweep;
pub use tags::normalize_tag;

use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct StoreOptions {
    pub max_file_size: u64,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            max_file_size: 200_000_000,
        }
    }
}

pub struct Store {
    blobs: PathBuf,
    staging_lock_path: PathBuf,
    options: StoreOptions,
    quota: FileQuota,
    writer: Mutex<Connection>,
    readers: [Mutex<Connection>; 2],
    schema_version: u32,
    cursor_key: Vec<u8>,
    #[cfg(test)]
    durability_events: Mutex<Vec<&'static str>>,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        Self::open_with(dir, StoreOptions::default())
    }

    pub fn open_with(dir: &Path, options: StoreOptions) -> Result<Self, StoreError> {
        Self::open_with_quota(dir, options, FileQuota::default())
    }

    pub fn open_with_quota(
        dir: &Path,
        options: StoreOptions,
        quota: FileQuota,
    ) -> Result<Self, StoreError> {
        let blobs = dir.join("blobs");
        fs::create_dir_all(&blobs)?;
        let blobs = blobs.canonicalize()?;
        let database = dir.join("store.db");
        let mut writer = Connection::open(&database)?;
        configure(&writer)?;
        let journal: String =
            writer.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !journal.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Invalid(
                "Could not enable SQLite WAL mode".into(),
            ));
        }
        let schema_version = schema::migrate(&mut writer)?;

        let cursor_key =
            writer.query_row("SELECT secret FROM cursor_secret", [], |row| row.get(0))?;

        let open_reader = || {
            let reader = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            configure(&reader)?;
            Ok::<_, StoreError>(Mutex::new(reader))
        };

        Ok(Self {
            blobs,
            staging_lock_path: dir.canonicalize()?.join(".blob-sweep-lock"),
            options,
            quota,
            writer: Mutex::new(writer),
            readers: [open_reader()?, open_reader()?],
            schema_version,
            cursor_key,
            #[cfg(test)]
            durability_events: Mutex::default(),
        })
    }

    fn reader(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        for reader in &self.readers {
            match reader.try_lock() {
                Ok(connection) => return Ok(connection),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Poisoned(_)) => return Err(StoreError::Conflict),
            }
        }
        self.readers[0].lock().map_err(|_| StoreError::Conflict)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

fn configure(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", true)
}
