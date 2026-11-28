use crate::{Store, StoreError};
use std::collections::HashSet;
use std::fs::{self, DirEntry, File, OpenOptions};
use std::path::PathBuf;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlobSweep {
    pub orphans: Vec<PathBuf>,
    pub temporary: Vec<PathBuf>,
}

impl Store {
    pub(crate) fn blob_staging_lock(&self) -> Result<File, StoreError> {
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.staging_lock_path)?)
    }

    /// Reports orphan blobs and interrupted uploads, optionally removing them.
    pub fn sweep_blobs(&self, fix: bool) -> Result<BlobSweep, StoreError> {
        // Acquire the staging lock before the database lock, as uploads do.
        let staging_lock = self.blob_staging_lock()?;
        staging_lock.lock()?;
        self.write(|tx| {
            let mut statement =
                tx.prepare("SELECT DISTINCT sha256 FROM files WHERE status = 'active'")?;
            let live = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;

            let mut report = BlobSweep::default();
            for entry in fs::read_dir(&self.blobs)? {
                let entry = entry?;
                let name = entry.file_name();
                let kind = entry.file_type()?;
                if kind.is_file() && is_temporary(&entry) {
                    report.temporary.push(entry.path());
                } else if kind.is_dir() && name.to_str().is_some_and(|name| is_hex(name, 2)) {
                    for blob in fs::read_dir(entry.path())? {
                        report.inspect_blob(blob?, &name.to_string_lossy(), &live)?;
                    }
                }
            }
            report.orphans.sort_unstable();
            report.temporary.sort_unstable();
            if fix {
                for path in report.orphans.iter().chain(&report.temporary) {
                    fs::remove_file(path)?;
                }
            }
            Ok(report)
        })
    }
}

impl BlobSweep {
    fn inspect_blob(
        &mut self,
        entry: DirEntry,
        shard: &str,
        live: &HashSet<String>,
    ) -> Result<(), StoreError> {
        if entry.file_type()?.is_file() {
            if is_temporary(&entry) {
                self.temporary.push(entry.path());
            } else if entry.file_name().to_str().is_some_and(|name| {
                is_hex(name, 64) && (!name.starts_with(shard) || !live.contains(name))
            }) {
                self.orphans.push(entry.path());
            }
        }
        Ok(())
    }
}

fn is_hex(name: &str, len: usize) -> bool {
    name.len() == len && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_temporary(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| name.starts_with(".tmp") || name.ends_with(".tmp"))
}
