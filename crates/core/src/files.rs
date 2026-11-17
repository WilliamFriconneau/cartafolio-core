//! Files kept in a space: photos and documents (an invoice, a manual, a photo of
//! the fuse box). The bytes live next to the databases, named by their SHA-256
//! (`files/<space>/<sha256>`, 0600), so the same file is stored once. Each file
//! gets a fact describing it, so `recall` finds it by its name or note. The type
//! comes from the content, never from the declared name. Nothing is deleted: a
//! file is retired and stays on disk. The one exception is [`erase`]: a voice memo kept as
//! audio in the inbox, once its transcription has been written for seven days.

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

#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    pub id: String,
    pub space: Space,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub sha256: String,
    pub note: String,
    pub fact_id: Option<String>,
    pub author: String,
    pub created_at: i64,
    pub status: String,
}

impl FileInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "space": self.space.as_str(), "name": self.name, "mime": self.mime,
            "size": self.size, "note": self.note, "fact_id": self.fact_id, "author": self.author,
            "created_at": iso(self.created_at), "status": self.status,
        })
    }

    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

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
    }
    Ok(())
}

/// Removes the bytes of a stored file from this space's folder (a window replica pruning, see `sync`).
pub(crate) fn remove_blob(brain: &Brain, space: &Space, sha: &str) -> Result<()> {
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
    let name = clean_name(name);
    if name.is_empty() {
        return Err(Error::Invalid("a file needs a name".into()));
    }
    let mime = sniff(bytes, &name).ok_or_else(|| {
        Error::Invalid(
            "only images (JPEG, PNG, GIF, WebP, HEIC), PDF, text, .xlsx workbooks and recordings (m4a, mp3, wav, ogg, webm, flac, caf, amr) are kept"
                .into(),
        )
    })?;
    let note = text::normalize(
        &note
            .chars()
            .filter(|c| !c.is_control())
            .take(1000)
            .collect::<String>(),
    );
    let conn = brain.open_space(space)?;
    let sha = hex(&Sha256::digest(bytes));

    // The same file under the same name is one file.
    let same: Option<String> = conn
        .query_row(
            "SELECT id FROM files WHERE sha256 = ?1 AND name = ?2 AND status = 'active' LIMIT 1",
            params![sha, name],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = same {
        return info(brain, space, &id)?
            .ok_or_else(|| Error::Schema("file not found after reading".into()));
    }

    // The bytes: written once per content, private.
    store_blob(brain, space, &sha, bytes)?;

    // A fact that makes it findable; it stands alone, distinct from close notes.
    let text = if note.is_empty() {
        format!("File: {name}")
    } else {
        format!("File: {name} — {note}")
    };
    let mut fact = NewFact::agent(&text, &format!("agent:{author}"));
    fact.kind = "context".into();
    fact.tags = tags.to_vec();
    let mut decided: Vec<String> = vec![];
    let fact_id = loop {
        match facts::remember(&conn, space, author, fact.clone(), &decided)? {
            Remembered::Created(f) | Remembered::Duplicate(f) => break Some(f.id),
            Remembered::NeedsDecision(c) if decided.is_empty() => {
                decided = c.into_iter().map(|c| c.id).collect()
            }
            Remembered::NeedsDecision(_) => break None,
        }
    };
    let id = format!("file_{}", util::random_base32(10));
    conn.execute(
        "INSERT INTO files (id, name, mime, size, sha256, note, fact_id, author, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            name,
            mime,
            i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            sha,
            note,
            fact_id,
            author,
            util::now()
        ],
    )?;
    if let Some(f) = &fact_id {
        conn.execute(
            "UPDATE facts SET type = 'file', fields = ?2 WHERE id = ?1",
            params![f, json!({ "file_id": id }).to_string()],
        )?;
    }
    crate::sync::record_file_put(&conn, &id)?;
    info(brain, space, &id)?.ok_or_else(|| Error::Schema("file not found after writing".into()))
}

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

/// The record and the bytes of a file.
pub fn get(brain: &Brain, space: &Space, id: &str) -> Result<Option<(FileInfo, Vec<u8>)>> {
    let Some(f) = info(brain, space, id)? else {
        return Ok(None);
    };
    if erased(brain, space, id)? {
        return Ok(None);
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
    conn.execute(
        "UPDATE files SET status = 'retired', retired_reason = ?2 WHERE id = ?1",
        params![id, reason],
    )?;
    if let Some(fact) = &f.fact_id {
        let _ = facts::retire(&conn, author, fact, &format!("file retired: {reason}"));
    }
    crate::sync::record_file_retired(&conn, id, reason)?;
    info(brain, space, id)?.ok_or_else(|| Error::Invalid("unknown file".into()))
}

/// How long a recording is kept after its transcription was written (seven days).
pub const ERASE_AFTER: i64 = 7 * 24 * 3600;

const ERASED: &str = "erased: ";

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
        .is_some_and(|reason| reason.starts_with(ERASED)))
}

/// Erases a voice memo for good, seven days after the text that replaces it was written.
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
            "the transcript is not a fact of a vault you can read".into(),
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
            "a recording is erased 7 days after its transcription was written".into(),
        ));
    }
    let reason = format!("{ERASED}transcribed in {transcript}, recording erased by {author}");
    conn.execute(
        "UPDATE files SET status = 'retired', retired_reason = ?2 WHERE id = ?1",
        params![id, reason],
    )?;
    if let Some(fact) = &f.fact_id {
        let _ = facts::retire(&conn, author, fact, &format!("file {reason}"));
    }
    crate::sync::record_file_retired(&conn, id, &reason)?;
    // The bytes go with the last record that holds them.
    let others: i64 = conn.query_row(
        "SELECT count(*) FROM files WHERE sha256 = ?1 AND id <> ?2
         AND NOT (status = 'retired' AND retired_reason LIKE 'erased: %')",
        params![f.sha256, id],
        |r| r.get(0),
    )?;
    if others == 0 {
        match fs::remove_file(blob_dir(brain, space).join(&f.sha256)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    info(brain, space, id)?.ok_or_else(|| Error::Invalid("unknown file".into()))
}
