use crate::{Store, StoreError};
use std::fs::{File, OpenOptions};

#[derive(Clone, Copy, Debug)]
pub struct FileQuota {
    pub max_bytes: u64,
    pub max_files: u64,
}

impl Default for FileQuota {
    fn default() -> Self {
        Self {
            max_bytes: 20_000_000_000,
            max_files: 100_000,
        }
    }
}

#[derive(serde::Serialize)]
pub struct FileStorage {
    pub bytes: u64,
    pub files: u64,
    pub max_bytes: u64,
    pub max_files: u64,
}

impl FileStorage {
    pub(crate) fn can_reserve(&self, bytes: u64) -> bool {
        self.files < self.max_files
            && self.bytes <= self.max_bytes
            && bytes <= self.max_bytes - self.bytes
    }
}

impl Store {
    pub fn file_storage(&self) -> Result<FileStorage, StoreError> {
        let reader = self.reader()?;
        let (bytes, files) = reader.query_row(
            "SELECT COALESCE(SUM(size) FILTER (WHERE status = 'active'), 0), COUNT(*) FROM files",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(FileStorage {
            bytes,
            files,
            max_bytes: self.quota.max_bytes,
            max_files: self.quota.max_files,
        })
    }

    pub(crate) fn reserve_upload(&self) -> Result<File, StoreError> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.staging_lock_path.with_file_name(".file-quota-lock"))?;
        lock.lock()?;
        Ok(lock)
    }
}

pub(crate) fn exceeded() -> StoreError {
    StoreError::Invalid("Account file storage quota exceeded".into())
}
