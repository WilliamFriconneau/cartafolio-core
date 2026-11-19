//! Files kept in a space: photos and documents (an invoice, a manual, a photo of
//! the fuse box). The bytes live next to the databases, named by their SHA-256
//! (`files/<space>/<sha256>`, 0600), so the same file is stored once. Each file
//! gets a fact describing it, so `recall` finds it by its name or note. The type
//! comes from the content, never from the declared name. Nothing is deleted: a
//! file is retired and stays on disk. The one exception is [`erase`]: a voice memo kept as
//! audio in the inbox, once its transcription has been written.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::facts::{self, NewFact, Remembered};
use crate::fsx::{PrivateDir as _, PrivateFile as _};
use crate::space::Space;
use crate::text::{self, hex};
use crate::util::{self, iso};

/// A stored file as listed: the record, never the bytes. The bytes are named by their digest on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    /// Opaque identifier of the record.
    pub id: String,
    /// The space that holds it.
    pub space: Space,
    /// Cleaned file name (no folders).
    pub name: String,
    /// Type read from the content (never from the name).
    pub mime: String,
    /// Size in bytes.
    pub size: i64,
    /// SHA-256 of the bytes in lowercase hex.
    pub sha256: String,
    /// Free note given at `put` (1000 characters at most); searched with the name.
    pub note: String,
    /// The fact that describes the file, so `recall` finds it.
    pub fact_id: Option<String>,
    /// The connection that put it.
    pub author: String,
    /// When it was put (seconds since the epoch).
    pub created_at: i64,
    /// `active` or `retired`.
    pub status: String,
}

impl FileInfo {
    /// The JSON shape served to clients: `id`, `space`, `name`, `mime`, `size`, `note`, `fact_id`, `author`, `created_at` (ISO 8601) and `status`. The digest is left out.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "space": self.space.as_str(), "name": self.name, "mime": self.mime,
            "size": self.size, "note": self.note, "fact_id": self.fact_id, "author": self.author,
            "created_at": iso(self.created_at), "status": self.status,
        })
    }

    /// Whether the type is `image/*`.
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// Whether the type is `audio/*`.
    pub fn is_audio(&self) -> bool {
        self.mime.starts_with("audio/")
    }
}

/// The type of an `.xlsx` workbook.
/// Whether `mime` is one of the types this program keeps (`sniff` can name it).
pub(crate) fn is_kept_type(mime: &str) -> bool {
    mime == XLSX
        || ["image/", "audio/", "text/plain", "application/pdf"]
            .iter()
            .any(|p| mime.starts_with(p))
            && mime.len() <= 60
            && mime.bytes().all(|b| b.is_ascii_graphic())
}

/// The media type of an Excel workbook (`.xlsx`).
pub const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// The type of the content, from its first bytes. `None` for anything not accepted.
pub fn sniff(bytes: &[u8], name: &str) -> Option<&'static str> {
    let starts = |p: &[u8]| bytes.starts_with(p);
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if starts(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && matches!(&bytes[8..12], b"heic" | b"heix" | b"mif1" | b"heif")
    {
        return Some("image/heic");
    }
    if starts(b"%PDF-") {
        return Some("application/pdf");
    }
    if let Some(m) = sniff_audio(bytes, name) {
        return Some(m);
    }
    // A spreadsheet export (a bank statement): a zip container, accepted by name as well.
    if starts(b"PK\x03\x04") && name.to_lowercase().ends_with(".xlsx") {
        return Some(XLSX);
    }
    if !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok() {
        let n = name.to_lowercase();
        return Some(if n.ends_with(".md") || n.ends_with(".markdown") {
            "text/markdown"
        } else {
            "text/plain"
        });
    }
    None
}

/// A recording, from its first bytes: Apple's voice memos and most phones (`m4a`), `mp3`, `wav`,
/// `ogg` (Opus and Vorbis), `webm` (what a browser records), `flac`, Apple's `caf` and `amr`.
fn sniff_audio(bytes: &[u8], name: &str) -> Option<&'static str> {
    let starts = |p: &[u8]| bytes.starts_with(p);
    let lower = name.to_lowercase();
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        // `isom` and `mp4x` also name videos: by the name only, when it says a recording.
        if matches!(brand, b"M4A " | b"M4B ")
            || (matches!(
                brand,
                b"isom" | b"mp41" | b"mp42" | b"iso5" | b"iso6" | b"dash"
            ) && lower.ends_with(".m4a"))
        {
            return Some("audio/mp4");
        }
        return None;
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return Some("audio/wav");
    }
    if starts(b"OggS") {
        return Some("audio/ogg");
    }
    // What a browser records (Chrome, Edge): WebM with Opus. By the name, since a video is a WebM too.
    if starts(&[0x1A, 0x45, 0xDF, 0xA3])
        && lower.ends_with(".webm")
        && ["dictation", "voice", "recording", "audio"]
            .iter()
            .any(|w| lower.starts_with(w))
    {
        return Some("audio/webm");
    }
    if starts(b"fLaC") {
        return Some("audio/flac");
    }
    if starts(b"caff") {
        return Some("audio/x-caf");
    }
    if starts(b"#!AMR") {
        return Some("audio/amr");
    }
    // `mp3`: an ID3 tag or a frame header (11 bits set), and the name says so, since two bytes
    // alone would take too much for audio.
    if lower.ends_with(".mp3")
        && (starts(b"ID3") || (bytes.len() > 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0))
    {
        return Some("audio/mpeg");
    }
    None
}

fn file_of(space: &Space, r: &rusqlite::Row) -> rusqlite::Result<FileInfo> {
    Ok(FileInfo {
        id: r.get(0)?,
        space: space.clone(),
        name: r.get(1)?,
        mime: r.get(2)?,
        size: r.get(3)?,
        sha256: r.get(4)?,
        note: r.get(5)?,
        fact_id: r.get(6)?,
        author: r.get(7)?,
        created_at: r.get(8)?,
        status: r.get(9)?,
    })
}

const COLS: &str = "id, name, mime, size, sha256, note, fact_id, author, created_at, status";

fn blob_dir(brain: &Brain, space: &Space) -> PathBuf {
    brain.dir().join("files").join(space.as_str())
}

/// The other spaces' directories of blobs.
fn other_blob_dirs(brain: &Brain, space: Option<&Space>) -> Vec<PathBuf> {
    let Ok(dirs) = fs::read_dir(brain.dir().join("files")) else {
        return vec![];
    };
    dirs.flatten()
        .map(|d| d.path())
        .filter(|p| p.is_dir() && space.is_none_or(|s| p.file_name() != Some(s.as_str().as_ref())))
        .collect()
}

fn link_from_another_space(brain: &Brain, space: &Space, sha: &str, path: &Path) -> bool {
    other_blob_dirs(brain, Some(space))
        .iter()
        .map(|d| d.join(sha))
        .any(|from| from.exists() && fs::hard_link(&from, path).is_ok())
}

/// Writes the bytes of a file once per content, private. Already held in another
/// space, they are a hard link there: each space keeps its own path, so
/// retiring it in one space leaves the other intact, and nothing tells a
/// space what another holds.
pub(crate) fn store_blob(brain: &Brain, space: &Space, sha: &str, bytes: &[u8]) -> Result<()> {
    let dir = blob_dir(brain, space);
    DirBuilder::new()
        .recursive(true)
        .private_dir()
        .create(&dir)?;
    let path = dir.join(sha);
    if !path.exists() && !link_from_another_space(brain, space, sha, &path) {
        let tmp = dir.join(format!(".{sha}.tmp"));
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .private_file()
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, &path)?;
        crate::fsx::sync_dir(&dir)?;
    }
    Ok(())
}

/// Removes the bytes of a stored file from this space's folder, and the chunks of it kept aside (a window replica
/// pruning, a recording erased at its origin: see `sync`).
pub(crate) fn remove_blob(brain: &Brain, space: &Space, sha: &str) -> Result<()> {
    crate::chunks::drop_staged(brain, space, sha);
    match fs::remove_file(blob_dir(brain, space).join(sha)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The bytes of a stored file, checked against their digest; `None` when they are not on disk.
pub(crate) fn read_blob(brain: &Brain, space: &Space, sha: &str) -> Result<Option<Vec<u8>>> {
    match fs::read(blob_dir(brain, space).join(sha)) {
        Ok(b) if hex(&Sha256::digest(&b)) == sha => Ok(Some(b)),
        Ok(_) => Err(Error::Schema(format!("file {sha} is damaged on disk"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The biggest file a space keeps: 2 GiB. A file up to [`MAX_INLINE`] travels to the other nodes inside its event; a
/// bigger one travels in chunks (see `crate::chunks`), read and written a chunk at a time, never whole in memory. It
/// comes in from a file on disk ([`put_path`]: a watched folder, `memcp intake send`, a file the remote command line
/// uploads) or from a peer.
pub const MAX_BYTES: usize = 2 * 1024 * 1024 * 1024;

/// The biggest file read whole in memory: 24 MiB. It is the most a file may be when it comes inside one request (an
/// assistant's `file_put`, a file put on the page: base64, a third bigger, in a body of at most 34 MiB), when a space's
/// folder brings it, when it is read back whole (`file_get`, [`get`]), and when it travels inside its event to a node of
/// an earlier version, which takes no chunks.
pub const MAX_INLINE: usize = 24 * 1024 * 1024;

/// Where the bytes of the file of digest `sha` of `space` are, whether they are there or not.
pub fn blob_path(brain: &Brain, space: &Space, sha: &str) -> PathBuf {
    blob_dir(brain, space).join(sha)
}

/// Whether this node holds the bytes of `sha` in `space`: already there, or a hard link made to the same bytes held in
/// another space of this node (nothing travels then).
pub(crate) fn hold_or_link(brain: &Brain, space: &Space, sha: &str) -> Result<bool> {
    let path = blob_path(brain, space, sha);
    if path.exists() {
        return Ok(true);
    }
    let dir = blob_dir(brain, space);
    DirBuilder::new()
        .recursive(true)
        .private_dir()
        .create(&dir)?;
    Ok(link_from_another_space(brain, space, sha, &path))
}

/// Stores a file in a space and describes it with a fact. Returns its record.
pub fn put(
    brain: &Brain,
    space: &Space,
    author: &str,
    name: &str,
    note: &str,
    bytes: &[u8],
) -> Result<FileInfo> {
    put_tagged(brain, space, author, name, note, bytes, &[])
}

/// [`put`], with the tags (already normalised) the describing fact is filed under: an app
/// that deposits a file files it under `sources/<app>`.
pub fn put_tagged(
    brain: &Brain,
    space: &Space,
    author: &str,
    name: &str,
    note: &str,
    bytes: &[u8],
    tags: &[String],
) -> Result<FileInfo> {
    if bytes.is_empty() {
        return Err(Error::Invalid("the file is empty".into()));
    }
    if bytes.len() > MAX_INLINE {
        return Err(Error::Invalid(format!(
            "the file is over {} MB, the most a file may be when it comes in one piece; a bigger one (up to {} MB) comes from a watched folder or `memcp intake send`",
            MAX_INLINE / 1024 / 1024,
            MAX_BYTES / 1024 / 1024
        )));
    }
    let name = clean_name(name);
    if name.is_empty() {
        return Err(Error::Invalid("a file needs a name".into()));
    }
    let mime = sniff(bytes, &name).ok_or_else(not_kept)?;
    let sha = hex(&Sha256::digest(bytes));
    if let Some(same) = same_file(brain, space, &sha, &name)? {
        return Ok(same);
    }
    // The bytes: written once per content, private, and named by a record before the sweep of a node that starts
    // may look at them.
    let writing = crate::recover::writing(brain)?;
    store_blob(brain, space, &sha, bytes)?;
    keep(
        &writing,
        brain,
        space,
        author,
        &name,
        note,
        mime,
        &sha,
        bytes.len() as u64,
        tags,
    )
}

fn not_kept() -> Error {
    Error::Invalid(
        "only images (JPEG, PNG, GIF, WebP, HEIC), PDF, text, .xlsx workbooks and recordings (m4a, mp3, wav, ogg, webm, flac, caf, amr) are kept"
            .into(),
    )
}

fn same_id(conn: &rusqlite::Connection, sha: &str, name: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM files WHERE sha256 = ?1 AND name = ?2 AND status = 'active' LIMIT 1",
            params![sha, name],
            |r| r.get(0),
        )
        .optional()?)
}

/// The same file under the same name is one file: its record, when it is already here.
fn same_file(brain: &Brain, space: &Space, sha: &str, name: &str) -> Result<Option<FileInfo>> {
    let same = same_id(&brain.open_space(space)?, sha, name)?;
    match same {
        Some(id) => {
            Ok(Some(info(brain, space, &id)?.ok_or_else(|| {
                Error::Schema("file not found after reading".into())
            })?))
        }
        None => Ok(None),
    }
}

/// The type of a big file from its first bytes: as [`sniff`], a text cut in the middle of a character included.
pub(crate) fn sniff_head(head: &[u8], name: &str) -> Option<&'static str> {
    let head = match std::str::from_utf8(head) {
        Err(e) if e.error_len().is_none() => &head[..e.valid_up_to()],
        _ => head,
    };
    sniff(head, name)
}

/// How much of a big file is read to know its type.
const HEAD_BYTES: usize = 64 * 1024;

/// [`put_tagged`] for a file on disk, up to [`MAX_BYTES`]: one of [`MAX_INLINE`] or less is read as before; a bigger one
/// is copied a block at a time beside the space's files while its digest is computed, never held whole in memory, and
/// its type is read from its first bytes.
pub fn put_path(
    brain: &Brain,
    space: &Space,
    author: &str,
    name: &str,
    note: &str,
    path: &Path,
    tags: &[String],
) -> Result<FileInfo> {
    let len = fs::metadata(path)?.len();
    if len <= MAX_INLINE as u64 {
        let bytes = fs::read(path)?;
        return put_tagged(brain, space, author, name, note, &bytes, tags);
    }
    if len > MAX_BYTES as u64 {
        return Err(Error::Invalid(format!(
            "the file is over {} MB, the most a space keeps",
            MAX_BYTES / 1024 / 1024
        )));
    }
    let name = clean_name(name);
    if name.is_empty() {
        return Err(Error::Invalid("a file needs a name".into()));
    }
    let mut src = fs::File::open(path)?;
    let mut head = vec![0u8; HEAD_BYTES];
    let mut got = 0;
    while got < head.len() {
        match std::io::Read::read(&mut src, &mut head[got..])? {
            0 => break,
            n => got += n,
        }
    }
    head.truncate(got);
    let mime = sniff_head(&head, &name).ok_or_else(not_kept)?;
    // Copied beside the files of the space, the digest computed on the way.
    let writing = crate::recover::writing(brain)?;
    let dir = blob_dir(brain, space);
    DirBuilder::new()
        .recursive(true)
        .private_dir()
        .create(&dir)?;
    let tmp = dir.join(format!(".in-{}.tmp", util::random_base32(10)));
    let copied = (|| -> Result<(String, u64)> {
        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .private_file()
            .open(&tmp)?;
        let mut hasher = Sha256::new();
        hasher.update(&head);
        out.write_all(&head)?;
        let mut total = head.len() as u64;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = std::io::Read::read(&mut src, &mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_BYTES as u64 {
                return Err(Error::Invalid(
                    "the file grew past the most a space keeps".into(),
                ));
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
        out.sync_all()?;
        Ok((hex(&hasher.finalize()), total))
    })();
    let (sha, size) = match copied {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if let Some(same) = same_file(brain, space, &sha, &name)? {
        let _ = fs::remove_file(&tmp);
        return Ok(same);
    }
    let target = dir.join(&sha);
    if target.exists() || link_from_another_space(brain, space, &sha, &target) {
        let _ = fs::remove_file(&tmp);
    } else {
        fs::rename(&tmp, &target)?;
        crate::fsx::sync_dir(&dir)?;
    }
    keep(
        &writing, brain, space, author, &name, note, mime, &sha, size, tags,
    )
}

/// The record of a file whose bytes are kept: its describing fact, its row and its event, in one transaction, so that
/// a crash leaves the file kept and told to the peers, or not kept at all (its bytes, named by nothing, go at the next
/// start). The caller holds `writing` from the first byte written.
#[allow(clippy::too_many_arguments)]
fn keep(
    _writing: &crate::recover::Writing,
    brain: &Brain,
    space: &Space,
    author: &str,
    name: &str,
    note: &str,
    mime: &str,
    sha: &str,
    size: u64,
    tags: &[String],
) -> Result<FileInfo> {
    crate::failpoint::hit(crate::failpoint::at::FILE_PUT_BYTES)?;
    let note = text::normalize(
        &note
            .chars()
            .filter(|c| !c.is_control())
            .take(1000)
            .collect::<String>(),
    );
    // A file too big to travel whole travels in chunks: their digests are in its event from the start. Read before
    // the write lock is taken.
    let list = if size > MAX_INLINE as u64 {
        Some(crate::chunks::hashes_of(&blob_path(brain, space, sha))?)
    } else {
        None
    };
    let conn = brain.open_space(space)?;
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    // Put twice at once, or again after an answer that never came: one record.
    if let Some(id) = same_id(&tx, sha, name)? {
        drop(tx);
        return info(brain, space, &id)?
            .ok_or_else(|| Error::Schema("file not found after reading".into()));
    }

    // A fact that makes it findable; it stands alone, distinct from close notes.
    let text = if note.is_empty() {
        format!("File: {name}")
    } else {
        format!("File: {name} — {note}")
    };
    let mut fact = NewFact::agent(&text, &format!("agent:{author}"));
    fact.kind = Some("context".into());
    fact.tags = tags.to_vec();
    let mut decided: Vec<String> = vec![];
    let mut fingerprinted = false;
    let fact_id = loop {
        match facts::remember_in(&tx, space, author, fact.clone(), &decided)? {
            Remembered::Created(f) => break Some(f.id),
            // A note of the same words is already there: another file of this name, or a note someone wrote. It is
            // never taken over (its file link and its tags stay its own): this file gets its own note, told apart by
            // the start of its fingerprint.
            Remembered::Duplicate(_) if !fingerprinted => {
                fingerprinted = true;
                // The same mark a merge gives a file whose note is taken.
                fact.text = format!("{text} [{}]", sha.get(..8).unwrap_or(sha));
            }
            Remembered::Duplicate(_) => break None,
            Remembered::NeedsDecision(c) if decided.is_empty() => {
                decided = c.into_iter().map(|c| c.id).collect()
            }
            Remembered::NeedsDecision(_) => break None,
        }
    };
    let id = format!("file_{}", util::random_base32(10));
    tx.execute(
        "INSERT INTO files (id, name, mime, size, sha256, note, fact_id, author, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            name,
            mime,
            i64::try_from(size).unwrap_or(i64::MAX),
            sha,
            note,
            fact_id,
            author,
            util::now()
        ],
    )?;
    if let Some(f) = &fact_id {
        tx.execute(
            "UPDATE facts SET type = 'file', fields = ?2 WHERE id = ?1",
            params![f, json!({ "file_id": id }).to_string()],
        )?;
    }
    crate::sync::record_file_put_with(&tx, &id, list.as_deref())?;
    crate::failpoint::hit(crate::failpoint::at::FILE_PUT_COMMIT)?;
    tx.commit()?;
    info(brain, space, &id)?.ok_or_else(|| Error::Schema("file not found after writing".into()))
}

/// The record of a file by id, any status. `Ok(None)` if there is none in this space. Read-only.
pub fn info(brain: &Brain, space: &Space, id: &str) -> Result<Option<FileInfo>> {
    let conn = brain.open_space(space)?;
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM files WHERE id = ?1"),
            [id],
            |r| file_of(space, r),
        )
        .optional()?)
}

/// The record of a file and where its bytes are, for a file of any size read a block at a time (the page serves it
/// so). `None` when the record is not in this space, or its bytes were erased or are not here.
pub fn path_of(brain: &Brain, space: &Space, id: &str) -> Result<Option<(FileInfo, PathBuf)>> {
    let Some(f) = info(brain, space, id)? else {
        return Ok(None);
    };
    if erased(brain, space, id)? {
        return Ok(None);
    }
    let path = blob_path(brain, space, &f.sha256);
    Ok(path.exists().then_some((f, path)))
}

/// The record and the bytes of a file, up to [`MAX_INLINE`] (a bigger one is an error: see [`path_of`]).
pub fn get(brain: &Brain, space: &Space, id: &str) -> Result<Option<(FileInfo, Vec<u8>)>> {
    let Some(f) = info(brain, space, id)? else {
        return Ok(None);
    };
    if erased(brain, space, id)? {
        return Ok(None);
    }
    if f.size > MAX_INLINE as i64 {
        return Err(Error::Invalid(format!(
            "{} is {} MB, too big to read in one piece (at most {} MB): open it on the page",
            f.name,
            f.size / 1024 / 1024,
            MAX_INLINE / 1024 / 1024
        )));
    }
    let bytes = fs::read(blob_dir(brain, space).join(&f.sha256))?;
    if hex(&Sha256::digest(&bytes)) != f.sha256 {
        return Err(Error::Schema(format!("file {id} is damaged on disk")));
    }
    Ok(Some((f, bytes)))
}

/// Active files of a space, newest first, optionally matching words of the name or note.
pub fn list(
    brain: &Brain,
    space: &Space,
    query: Option<&str>,
    limit: usize,
) -> Result<Vec<FileInfo>> {
    let conn = brain.open_space(space)?;
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM files WHERE status = 'active' ORDER BY created_at DESC, id LIMIT 1000"
    ))?;
    let words: Vec<String> = query
        .map(|q| text::fold(q).split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    let rows = st.query_map([], |r| file_of(space, r))?;
    let mut out = vec![];
    for f in rows {
        let f = f?;
        let hay = text::fold(&format!("{} {}", f.name, f.note));
        if words.iter().all(|w| hay.contains(w.as_str())) {
            out.push(f);
        }
        if out.len() >= limit.max(1) {
            break;
        }
    }
    Ok(out)
}

/// Retires a file: out of lists and searches, the bytes stay on disk.
pub fn retire(
    brain: &Brain,
    space: &Space,
    author: &str,
    id: &str,
    reason: &str,
) -> Result<FileInfo> {
    if reason.trim().is_empty() {
        return Err(Error::Invalid("retiring needs a reason".into()));
    }
    let conn = brain.open_space(space)?;
    let f = info(brain, space, id)?.ok_or_else(|| Error::Invalid("unknown file".into()))?;
    // The record, its note and its event together: a crash leaves the file retired everywhere, or not at all.
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE files SET status = 'retired', retired_reason = ?2 WHERE id = ?1",
        params![id, reason],
    )?;
    if let Some(fact) = &f.fact_id {
        facts::retire_if_active(&tx, author, fact, &format!("file retired: {reason}"))?;
    }
    crate::sync::record_file_retired(&tx, id, reason, false)?;
    crate::sync::settle_disputes(&tx)?;
    crate::failpoint::hit(crate::failpoint::at::FILE_RETIRE_COMMIT)?;
    tx.commit()?;
    info(brain, space, id)?.ok_or_else(|| Error::Invalid("unknown file".into()))
}

/// The tag a recording whose transcription failed three times carries on its description: visible on the page and to an
/// assistant, and counted apart from what waits for the person. Taking it off is the retry.
pub const FAILED_TRANSCRIPTION: &str = "failed/transcription";

/// Puts the tag [`FAILED_TRANSCRIPTION`] on a recording's description, or takes it off (`failed` false: the node tries
/// the recording again). Only a recording that is still active; the tag is the only thing that changes.
pub fn mark_transcription_failed(
    brain: &Brain,
    space: &Space,
    id: &str,
    failed: bool,
) -> Result<FileInfo> {
    let f = info(brain, space, id)?
        .filter(|f| f.status == "active" && f.is_audio())
        .ok_or_else(|| Error::Invalid("no active recording under this id".into()))?;
    let fact = f
        .fact_id
        .as_deref()
        .ok_or_else(|| Error::Invalid("this recording has no description".into()))?;
    let conn = brain.open_space(space)?;
    if failed {
        conn.execute(
            "INSERT OR IGNORE INTO fact_tags (fact_id, tag) VALUES (?1, ?2)",
            params![fact, FAILED_TRANSCRIPTION],
        )?;
    } else {
        conn.execute(
            "DELETE FROM fact_tags WHERE fact_id = ?1 AND tag = ?2",
            params![fact, FAILED_TRANSCRIPTION],
        )?;
    }
    Ok(f)
}

/// How long a recording is kept after its transcription was written: not at all. Once its text exists, the audio
/// goes (the person's rule: a transcribed voice note is thrown away); the record, the reason and the text stay.
pub const ERASE_AFTER: i64 = 0;

const ERASED: &str = "erased: ";

/// Whether a retirement reason says the bytes were erased for good (see [`erase`]).
pub(crate) fn is_erased_reason(reason: &str) -> bool {
    reason.starts_with(ERASED)
}

/// The recording is gone for good (see [`erase`]); its record stays.
pub fn erased(brain: &Brain, space: &Space, id: &str) -> Result<bool> {
    let conn = brain.open_space(space)?;
    let reason: Option<Option<String>> = conn
        .query_row(
            "SELECT retired_reason FROM files WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(reason
        .flatten()
        .is_some_and(|reason| is_erased_reason(&reason)))
}

/// Erases a voice memo for good, once the text that replaces it is written.
pub fn erase(
    brain: &Brain,
    space: &Space,
    author: &str,
    id: &str,
    transcript_space: &Space,
    transcript: &str,
) -> Result<FileInfo> {
    erase_at(
        brain,
        space,
        author,
        id,
        transcript_space,
        transcript,
        util::now(),
    )
}

/// The tag of the note a transcription job leaves when it gave up on a recording: the recording's id, nothing else.
pub const VOICE_FAILED_TAG: &str = "inbox/audio/failed";

/// The recordings of a space that wait for a text: active, under `inbox/audio`, and named by no text yet (by name or by
/// id, a failure note not counting). `(id, name, created_at)`, oldest first.
pub fn recordings_waiting(conn: &rusqlite::Connection) -> Result<Vec<(String, String, i64)>> {
    let mut st = conn.prepare(
        "SELECT fl.id, fl.name, fl.created_at FROM files fl
         JOIN facts f ON f.id = fl.fact_id
         WHERE fl.status = 'active' AND fl.mime LIKE 'audio/%' AND f.status = 'active'
           AND EXISTS (SELECT 1 FROM fact_tags t WHERE t.fact_id = f.id AND t.tag = 'inbox/audio')
           AND NOT EXISTS (
             SELECT 1 FROM facts d WHERE d.status = 'active' AND d.type IS NOT 'file'
               AND (instr(d.text, fl.name) > 0 OR instr(d.text, fl.id) > 0)
               AND NOT EXISTS (SELECT 1 FROM fact_tags x WHERE x.fact_id = d.id AND x.tag = ?1))
         ORDER BY fl.created_at",
    )?;
    let rows = st.query_map([VOICE_FAILED_TAG], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// [`erase`] at a given instant. The one exception to « nothing is erased », and a narrow one:
/// only a recording filed under `inbox/audio` (waiting to be transcribed), only once an active
/// text fact, named by the caller, has existed for [`ERASE_AFTER`]. The transcription may have
/// been filed in another vault than the recording (it is sorted after it is written): the
/// caller says which, and the tool only looks where the key can read. The
/// bytes go unless another record of the vault still holds them; the record, its fact (retired)
/// and the reason (« erased: » and the id of the transcript) stay as the trace.
pub fn erase_at(
    brain: &Brain,
    space: &Space,
    author: &str,
    id: &str,
    transcript_space: &Space,
    transcript: &str,
    now: i64,
) -> Result<FileInfo> {
    let conn = brain.open_space(space)?;
    let f = info(brain, space, id)?.ok_or_else(|| Error::Invalid("unknown file".into()))?;
    if erased(brain, space, id)? {
        // Erased already (an answer that never came, a retry): done once. Its bytes go if a crash kept them.
        crate::recover::drop_unless_held(brain, space, &conn, &f.sha256)?;
        return Ok(f);
    }
    if f.status != "active" {
        return Err(Error::Invalid("this file is already retired".into()));
    }
    if !f.is_audio() {
        return Err(Error::Invalid("only a recording is erased".into()));
    }
    // Still waiting: its fact active (a recording someone sorted elsewhere by `supersede` is kept) and
    // still under inbox/audio.
    let waiting: bool = match &f.fact_id {
        Some(fact) => conn.query_row(
            "SELECT count(*) > 0 FROM fact_tags t JOIN facts f ON f.id = t.fact_id
             WHERE t.fact_id = ?1 AND t.tag = 'inbox/audio' AND f.status = 'active'",
            [fact],
            |r| r.get(0),
        )?,
        None => false,
    };
    if !waiting {
        return Err(Error::Invalid(
            "only a recording filed under inbox/audio is erased".into(),
        ));
    }
    let written: Option<(String, Option<String>, i64, String)> = brain
        .open_space(transcript_space)?
        .query_row(
            "SELECT status, type, created_at, text FROM facts WHERE id = ?1",
            [transcript],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((status, kind, at, text)) = written else {
        return Err(Error::Invalid(
            "the transcript is not a fact of a space you can read".into(),
        ));
    };
    if status != "active" || kind.as_deref() == Some("file") {
        return Err(Error::Invalid(
            "the transcript must be an active text fact, not a file".into(),
        ));
    }
    // The transcript is this recording's: written after it, and naming it (by its file name or its id).
    if at < f.created_at || !(text.contains(&f.name) || text.contains(&f.id)) {
        return Err(Error::Invalid(
            "the transcript must name this recording (its file name) and be written after it"
                .into(),
        ));
    }
    if now < at.saturating_add(ERASE_AFTER) {
        return Err(Error::Invalid(
            "a recording is erased once its transcription is written, not before".into(),
        ));
    }
    let reason = format!("{ERASED}transcribed in {transcript}, recording erased by {author}");
    // The record, its note and the event that tells every node to erase it too, in one transaction: a crash leaves
    // the recording erased and told, or untouched.
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE files SET status = 'retired', retired_reason = ?2 WHERE id = ?1 AND status = 'active'",
        params![id, reason],
    )?;
    if changed != 1 {
        return Err(Error::Invalid(
            "this file was just retired by another call".into(),
        ));
    }
    if let Some(fact) = &f.fact_id {
        facts::retire_if_active(&tx, author, fact, &format!("file {reason}"))?;
    }
    crate::sync::record_file_retired(&tx, id, &reason, true)?;
    crate::sync::settle_disputes(&tx)?;
    crate::failpoint::hit(crate::failpoint::at::FILE_ERASE_COMMIT)?;
    tx.commit()?;
    crate::failpoint::hit(crate::failpoint::at::FILE_ERASE_BYTES)?;
    // The bytes go with the last record that holds them; a crash before leaves them to the next start's sweep.
    crate::recover::drop_unless_held(brain, space, &conn, &f.sha256)?;
    info(brain, space, id)?.ok_or_else(|| Error::Invalid("unknown file".into()))
}
