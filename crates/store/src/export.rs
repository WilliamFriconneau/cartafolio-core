use crate::fact::{self, fact_from_row};
use crate::file::{self, blob_path, file_from_row, validate_name};
use crate::{Fact, FactStatus, FileInfo, Store, StoreError};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{self, BufReader, Cursor, Read, Seek};
use std::path::{Component, Path, PathBuf};
use tempfile::NamedTempFile;

impl Store {
    /// Exports notes and active attachments into an account-specific directory.
    pub fn export(&self, out: &Path) -> Result<(), StoreError> {
        let out: PathBuf = out.components().collect();
        fs::create_dir_all(&out)?;
        if !fs::symlink_metadata(&out)?.is_dir() {
            return Err(invalid_path());
        }
        let out = out.canonicalize()?;
        // Keep blobs referenced by the read snapshot available until copying finishes.
        let staging_lock = self.blob_staging_lock()?;
        staging_lock.lock_shared()?;
        let reader = self.reader()?;
        let snapshot = reader.unchecked_transaction()?;
        export_facts(&snapshot, &out)?;
        export_files(&snapshot, &out, &self.blobs)?;
        snapshot.commit()?;
        Ok(())
    }
}

fn export_facts(db: &Connection, out: &Path) -> Result<(), StoreError> {
    let mut statement = db.prepare(&format!(
        "SELECT {}, strftime('%Y-%m', created_at, 'unixepoch') FROM facts ORDER BY id",
        fact::COLUMNS
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let fact = fact_from_row(row)?;
        let month: Option<String> = row.get(13)?;
        let month = month.filter(|month| month.len() == 7);
        export_fact(out, &fact, month.as_deref())?;
    }
    Ok(())
}

fn export_fact(out: &Path, fact: &Fact, month: Option<&str>) -> Result<(), StoreError> {
    let space = space_dir(out, &fact.space)?;
    let name = record_filename(&fact.id)?;
    let folder = if fact.status == FactStatus::Active {
        month.ok_or_else(|| {
            StoreError::Invalid("Capture time cannot be exported as YYYY-MM".into())
        })?
    } else {
        "_history"
    };
    let destination = directory(&space, folder)?.join(&name);
    let markdown = markdown(
        [
            ("id", json!(fact.id)),
            ("tags", json!(fact.tags)),
            ("author", json!(fact.author)),
            ("source", json!(fact.source)),
            ("created", json!(fact.created_at)),
            ("pinned", json!(fact.pinned)),
            ("replaces", json!(fact.replaces)),
        ],
        &fact.text,
    );
    write_changed(&destination, Cursor::new(markdown))?;

    if let Some(month) = month.filter(|_| fact.status != FactStatus::Active) {
        let old_dir = space.join(month);
        if let Some(metadata) = existing_metadata(&old_dir)? {
            if !metadata.is_dir() {
                return Err(invalid_path());
            }
            let old_note = old_dir.join(name);
            if let Some(metadata) = existing_metadata(&old_note)? {
                if !metadata.is_file() {
                    return Err(invalid_path());
                }
                fs::remove_file(old_note)?;
            }
        }
    }
    Ok(())
}

fn export_files(db: &Connection, out: &Path, blobs: &Path) -> Result<(), StoreError> {
    let mut statement = db.prepare(&format!(
        "SELECT {} FROM files WHERE status = 'active' ORDER BY id",
        file::COLUMNS
    ))?;
    for file in statement.query_map([], file_from_row)? {
        export_file(out, blobs, &file?)?;
    }
    Ok(())
}

fn export_file(out: &Path, blobs: &Path, file: &FileInfo) -> Result<(), StoreError> {
    validate_name(&file.name)?;
    let sidecar = record_filename(&file.id)?;
    let space = space_dir(out, &file.space)?;
    let folder = directory(&space, "_files")?;
    let attachment = directory(&folder, &file.id)?.join(component(&file.name)?);
    let source = File::open(blob_path(blobs, &file.sha256)?)?;
    write_changed(&attachment, BufReader::new(source))?;

    let markdown = markdown(
        [
            ("id", json!(file.id)),
            ("name", json!(file.name)),
            ("mime", json!(file.mime)),
            ("created", json!(file.created_at)),
            ("sha256", json!(file.sha256)),
        ],
        file.note.as_deref().unwrap_or_default(),
    );
    write_changed(&folder.join(sidecar), Cursor::new(markdown))
}

fn markdown<const N: usize>(fields: [(&str, Value); N], text: &str) -> String {
    let mut output = String::from("---\n");
    for (key, value) in fields {
        output.push_str(key);
        output.push_str(": ");
        output.push_str(&value.to_string());
        output.push('\n');
    }
    output.push_str("---\n");
    output.push_str(text);
    output
}

fn record_filename(id: &str) -> Result<String, StoreError> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_path());
    }
    Ok(format!("{id}.md"))
}

fn space_dir(out: &Path, space: &str) -> Result<PathBuf, StoreError> {
    if !(2..=40).contains(&space.len())
        || !space
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(invalid_path());
    }
    directory(out, space)
}

fn component(name: &str) -> Result<&Path, StoreError> {
    let path = Path::new(name);
    match path.components().next() {
        Some(Component::Normal(part)) if part == path.as_os_str() => Ok(path),
        _ => Err(invalid_path()),
    }
}

fn directory(parent: &Path, name: &str) -> Result<PathBuf, StoreError> {
    let path = parent.join(component(name)?);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&path)?.is_dir() {
                return Err(invalid_path());
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

fn existing_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn invalid_path() -> StoreError {
    StoreError::Invalid("Unsafe export path".into())
}

fn write_changed(path: &Path, mut source: impl Read + Seek) -> Result<(), StoreError> {
    if let Some(metadata) = existing_metadata(path)? {
        if !metadata.is_file() {
            return Err(invalid_path());
        }
        if same_bytes(&mut source, BufReader::new(File::open(path)?))? {
            return Ok(());
        }
        source.rewind()?;
    }
    let parent = path.parent().ok_or_else(invalid_path)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    io::copy(&mut source, &mut temp)?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn same_bytes(source: &mut impl Read, mut existing: impl Read) -> io::Result<bool> {
    let mut expected = [0; 8192];
    let mut actual = [0; 8192];
    loop {
        let count = source.read(&mut expected)?;
        if count == 0 {
            return Ok(existing.read(&mut actual[..1])? == 0);
        }
        match existing.read_exact(&mut actual[..count]) {
            Ok(()) if expected[..count] == actual[..count] => {}
            Ok(()) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error),
        }
    }
}
