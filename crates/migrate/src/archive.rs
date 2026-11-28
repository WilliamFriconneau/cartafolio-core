use crate::rpc::Result;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub struct Archive {
    manifest: NamedTempFile,
    path: PathBuf,
    blobs: PathBuf,
}

impl Archive {
    pub fn new(root: &Path, space: &str) -> Result<Self> {
        let blobs = root.join("files");
        fs::create_dir_all(&blobs).map_err(io_error)?;
        let name = if space
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            space.to_owned()
        } else {
            use sha2::{Digest, Sha256};
            format!("@{:x}", Sha256::digest(space.as_bytes()))
        };
        Ok(Self {
            manifest: NamedTempFile::new_in(root).map_err(io_error)?,
            path: root.join(format!("{name}.jsonl")),
            blobs,
        })
    }

    pub fn entry(&mut self, kind: &str, record: &Value) -> Result<()> {
        serde_json::to_writer(&mut self.manifest, &json!({"type":kind,"record":record}))
            .map_err(|_| "Could not serialize archive record")?;
        self.manifest.write_all(b"\n").map_err(io_error)
    }

    pub fn file(&mut self, record: &Value, digest: &str, bytes: &[u8]) -> Result<()> {
        let mut blob = NamedTempFile::new_in(&self.blobs).map_err(io_error)?;
        blob.write_all(bytes).map_err(io_error)?;
        blob.persist(self.blobs.join(digest))
            .map_err(|error| io_error(error.error))?;
        self.entry(
            "file",
            &json!({"metadata":record,"blob":format!("files/{digest}")}),
        )
    }

    pub fn finish(self) -> Result<()> {
        self.manifest
            .persist(self.path)
            .map_err(|error| io_error(error.error))?;
        Ok(())
    }
}

fn io_error(error: std::io::Error) -> String {
    format!("Could not write archive ({})", error.kind())
}
