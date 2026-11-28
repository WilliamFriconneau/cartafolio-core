#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(test)]
mod open_tests;
#[cfg(test)]
mod search_tests;

mod error;
mod fact;
mod file;
mod list;
mod schema;
mod search;
mod space;
mod tags;

pub use error::StoreError;
pub use fact::{Fact, FactStatus, NewFact, Outcome, Remembered};
pub use file::{FileInfo, FileStatus, NewFile};
pub use list::{ListQuery, Page};
pub use search::{Hit, RecallQuery};
pub use space::SpaceInfo;
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
    options: StoreOptions,
    writer: Mutex<Connection>,
    readers: [Mutex<Connection>; 2],
    schema_version: u32,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        Self::open_with(dir, StoreOptions::default())
    }

    pub fn open_with(dir: &Path, options: StoreOptions) -> Result<Self, StoreError> {
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

        let open_reader = || {
            let reader = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            configure(&reader)?;
            Ok::<_, StoreError>(Mutex::new(reader))
        };

        Ok(Self {
            blobs,
            options,
            writer: Mutex::new(writer),
            readers: [open_reader()?, open_reader()?],
            schema_version,
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
