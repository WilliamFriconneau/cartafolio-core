//! A folder of this node for a space, kept in step with it in both directions.
//!
//! Every space can point, **on each node**, to a local folder of the person's choice. The node keeps the space's notes
//! there as Markdown (`<slug>-<shortid>.md`, with a front matter) and its files as themselves (in `files/`). A change
//! made in the folder comes back into the memory as a new version of the note (`supersede`) or a new file; a new `.md`
//! or file becomes a note or a file of the space. See `docs/FOLDERS.md` for the rules.
//!
//! The setting is **per node**: it lives in the control database (`space_folders`, `folder_state`), which no exchange
//! between nodes ever carries. A path of one machine means nothing on another.
//!
//! Safety rules, all tested: the node writes only the files it made (a name already taken by a file it did not make is
//! never overwritten); a file it wrote is not read back as a change (hash of what it wrote); when a note changed on
//! both sides since the last pass the two are kept (the folder copy becomes a new note tagged `conflict`); a file
//! deleted from the folder retires its note with a reason, but a note retired in the memory never deletes a file; a
//! folder that vanishes (a drive unmounted) retires nothing; in `read` mode nothing is written, in either direction.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::Brain;
use crate::error::{Error, Result};
use crate::facts::{self, Fact, NewFact, Remembered};
use crate::files;
use crate::library;
use crate::space::Space;
use crate::text::{self, hex};
use crate::util;

/// The author of everything the folder engine writes into the memory.
pub const AUTHOR: &str = "folder-sync";
/// The tag of the note that keeps the folder's copy when a note changed on both sides.
pub const CONFLICT_TAG: &str = "conflict";
/// The subfolder that holds the space's files.
pub const FILES_DIR: &str = "files";

/// A file of the folder bigger than this is left alone: the engine compares a file's content in memory, so it takes
/// what is read whole (see `files::MAX_INLINE`). A bigger file of the space (one that came from a peer or a watched
/// folder) is still written to the folder, copied a block at a time.
const MAX_FILE: i64 = files::MAX_INLINE as i64;
/// Deepest subfolder the engine goes into.
const MAX_DEPTH: usize = 12;
/// Most files one folder may hold for the engine.
const MAX_ENTRIES: usize = 200_000;
/// Most error lines kept for the page.
const MAX_ERRORS: usize = 50;
/// Extensions that say a file is not finished.
const UNFINISHED: [&str; 7] = [
    "part",
    "crdownload",
    "tmp",
    "download",
    "partial",
    "swp",
    "memcp-tmp",
];

/// What the node does with the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Both directions: the folder shows the notes and files, and what is changed there comes back.
    ReadWrite,
    /// The folder is only read (indexed for search): nothing is written into it, nothing comes into the memory from it.
    Read,
}

impl Mode {
    /// `readwrite` or `read`.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::ReadWrite => "readwrite",
            Mode::Read => "read",
        }
    }

    /// The inverse of [`Mode::as_str`].
    pub fn parse(s: &str) -> Result<Mode> {
        match s {
            "readwrite" => Ok(Mode::ReadWrite),
            "read" => Ok(Mode::Read),
            _ => Err(Error::Invalid("mode: readwrite or read".into())),
        }
    }
}

/// The folder set for a space on this node.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    /// The space.
    pub space: Space,
    /// Absolute path, as the node resolved it.
    pub path: PathBuf,
    /// What the node does with it.
    pub mode: Mode,
    /// The cloud service that holds it, if the path says so ([`cloud_of`]).
    pub cloud: Option<String>,
    /// When it was set.
    pub set_at: i64,
    /// End of the last pass, if any.
    pub last_scan: Option<i64>,
    /// Files and notes tracked at the end of the last pass.
    pub files: i64,
    /// What went wrong at the last pass.
    pub errors: Vec<String>,
}

/// How a pass went.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Notes and files written into the folder (new or updated).
    pub written: usize,
    /// Notes (and files) that got a new version from an edit in the folder.
    pub edited: usize,
    /// New notes made from files of the folder.
    pub new_notes: usize,
    /// New files made from files of the folder.
    pub new_files: usize,
    /// Notes changed on both sides: the folder copy was kept as a note tagged `conflict`.
    pub conflicts: usize,
    /// Notes and files retired because their file was deleted from the folder.
    pub retired: usize,
    /// Files not taken yet (still being written).
    pub waiting: usize,
    /// What went wrong, one line each.
    pub errors: Vec<String>,
}

impl Report {
    /// The JSON shape: the counts and `errors`.
    pub fn to_json(&self) -> Value {
        json!({
            "written": self.written, "edited": self.edited, "new_notes": self.new_notes, "new_files": self.new_files,
            "conflicts": self.conflicts, "retired": self.retired, "waiting": self.waiting, "errors": self.errors,
        })
    }
}

/// How a pass is run.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// A file whose last change is younger than this many seconds is still being written: next time.
    pub settle_secs: i64,
}

impl Default for Options {
    fn default() -> Self {
        Options { settle_secs: 3 }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Pure helpers: cloud detection, slug, front matter, hashing.

/// The cloud service that holds a folder, read from its path: `google-drive`, `icloud`, `onedrive`, `dropbox`, or
/// `None`. Only the names of the folders on the way are read (case ignored); the disk is not touched.
pub fn cloud_of(path: &Path) -> Option<&'static str> {
    for c in path.components() {
        let c = c.as_os_str().to_string_lossy().to_lowercase();
        if c.starts_with("googledrive-")
            || c == "google drive"
            || c == "googledrive"
            || c == "my drive"
        {
            return Some("google-drive");
        }
        if c == "com~apple~clouddocs"
            || c == "mobile documents"
            || c == "icloud drive"
            || c == "iclouddrive"
        {
            return Some("icloud");
        }
        if c == "onedrive" || c.starts_with("onedrive -") || c.starts_with("onedrive-") {
            return Some("onedrive");
        }
        if c == "dropbox" || c.starts_with("dropbox (") || c.starts_with("dropbox-") {
            return Some("dropbox");
        }
    }
    None
}

/// The advice shown for a folder held by a cloud service: keep it on the device. `None` for a plain folder.
pub fn cloud_warning(cloud: Option<&str>) -> Option<String> {
    let name = match cloud? {
        "google-drive" => "Google Drive",
        "icloud" => "iCloud Drive",
        "onedrive" => "OneDrive",
        "dropbox" => "Dropbox",
        other => other,
    };
    Some(format!(
        "This folder is held by {name}. Keep it on the device (always available offline): a file the cloud has not downloaded cannot be read, and two programs writing the same files make copies and conflicts. It works, but a folder that stays on the device is more reliable."
    ))
}

/// The folder a space uses on this node when none was chosen: the node's own storage folder for it.
pub fn default_path(brain: &Brain, space: &Space) -> PathBuf {
    brain.dir().join(FILES_DIR).join(space.as_str())
}

/// A readable name from a text: lowercase ASCII, words joined by `-`, at most 48 characters (`note` if nothing is left).
pub fn slug(text: &str) -> String {
    let folded = text::fold(text);
    let mut out = String::new();
    for c in folded.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "note".into() } else { out }
}

/// The short form of a fact id used in a file name: the first six characters after the prefix.
pub fn short_id(id: &str) -> String {
    let tail = id.split_once('_').map_or(id, |(_, t)| t);
    tail.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(6)
        .collect::<String>()
        .to_lowercase()
}

/// Lowercase hex SHA-256 of some bytes.
pub fn hash_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// What a note file says: its `id` and `tags` if the front matter has them, and its body.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// The `id` key of the front matter.
    pub id: Option<String>,
    /// The `tags` key; `None` when there is none (the note keeps the tags it has).
    pub tags: Option<Vec<String>>,
    /// The text after the front matter.
    pub body: String,
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches(['"', '\'']).trim()
}

/// Reads a note file: an optional front matter between two `---` lines, then the text. Pure.
pub fn parse_note(raw: &str) -> Parsed {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let lines: Vec<&str> = raw.lines().collect();
    let none = || Parsed {
        id: None,
        tags: None,
        body: raw.to_string(),
    };
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return none();
    }
    let Some(end) = lines[1..].iter().position(|l| l.trim_end() == "---") else {
        return none();
    };
    let head = &lines[1..=end];
    let body = lines[end + 2..].join("\n");
    let mut id = None;
    let mut tags: Option<Vec<String>> = None;
    let mut in_tags = false;
    for l in head {
        if in_tags && let Some(item) = l.trim_start().strip_prefix("- ") {
            if let Some(t) = tags.as_mut() {
                t.push(unquote(item).to_string());
            }
            continue;
        }
        in_tags = false;
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        match k.trim() {
            "id" => id = Some(unquote(v).to_string()).filter(|s| !s.is_empty()),
            "tags" => {
                let v = v.trim();
                if v.is_empty() {
                    in_tags = true;
                    tags = Some(vec![]);
                } else {
                    let inner = v.trim_start_matches('[').trim_end_matches(']');
                    tags = Some(
                        inner
                            .split(',')
                            .map(|t| unquote(t).to_string())
                            .filter(|t| !t.is_empty())
                            .collect(),
                    );
                }
            }
            _ => {}
        }
    }
    Parsed { id, tags, body }
}

/// The Markdown file of a note: front matter (`id`, `space`, `tags`, `created`, `version`) then the text. Pure.
pub fn render_note(f: &Fact, created: i64, version: usize) -> String {
    let mut tags = f.tags.clone();
    tags.sort();
    format!(
        "---\nid: {}\nspace: {}\ntags: [{}]\ncreated: {}\nversion: {}\n---\n\n{}\n",
        f.id,
        f.space,
        tags.join(", "),
        util::iso(created),
        version,
        f.text
    )
}

fn sorted(tags: &[String]) -> Vec<String> {
    let mut t = tags.to_vec();
    t.sort();
    t
}

// ---------------------------------------------------------------------------------------------------------------
// The setting.

const FOLDER_COLS: &str = "space, path, mode, cloud, set_at, last_scan, last_files, last_errors";

fn folder_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<Folder> {
    let bad = |i: usize, e: Error| {
        rusqlite::Error::FromSqlConversionFailure(
            i,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    };
    let space: String = r.get(0)?;
    let path: String = r.get(1)?;
    let mode: String = r.get(2)?;
    let errors: String = r.get(7)?;
    Ok(Folder {
        space: Space::new(&space).map_err(|e| bad(0, e))?,
        path: PathBuf::from(path),
        mode: Mode::parse(&mode).map_err(|e| bad(2, e))?,
        cloud: r.get(3)?,
        set_at: r.get(4)?,
        last_scan: r.get(5)?,
        files: r.get(6)?,
        errors: serde_json::from_str(&errors).unwrap_or_default(),
    })
}

fn read_row(ctl: &Connection, space: &Space) -> Result<Option<Folder>> {
    Ok(ctl
        .query_row(
            &format!("SELECT {FOLDER_COLS} FROM space_folders WHERE space = ?1"),
            [space.as_str()],
            folder_of,
        )
        .optional()?)
}

/// The folder set for a space on this node, if one was.
pub fn get(brain: &Brain, space: &Space) -> Result<Option<Folder>> {
    read_row(&brain.open_control()?, space)
}

/// Every folder set on this node.
pub fn all(brain: &Brain) -> Result<Vec<Folder>> {
    let ctl = brain.open_control()?;
    let mut st = ctl.prepare(&format!(
        "SELECT {FOLDER_COLS} FROM space_folders ORDER BY space"
    ))?;
    let rows = st.query_map([], folder_of)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// What the setting of a space says, as served to clients: `space`, `path` (the folder in use: the one chosen, else the
/// default), `default_path`, `custom` (whether a folder was chosen), `mode` (`readwrite`, `read`, or `null` when none
/// was chosen), `cloud` (`google-drive`, `icloud`, `onedrive`, `dropbox` or `null`), `warning` (advice for a folder held
/// by a cloud service, else `null`), `last_scan` (ISO 8601 or `null`), `files` (notes and files tracked) and `errors`.
pub fn info(brain: &Brain, space: &Space) -> Result<Value> {
    let default = default_path(brain, space).display().to_string();
    Ok(match get(brain, space)? {
        Some(f) => json!({
            "space": space.as_str(), "path": f.path.display().to_string(), "default_path": default,
            "custom": true, "mode": f.mode.as_str(), "cloud": f.cloud, "warning": cloud_warning(f.cloud.as_deref()),
            "last_scan": f.last_scan.map(util::iso), "files": f.files, "errors": f.errors,
        }),
        None => json!({
            "space": space.as_str(), "path": default, "default_path": default,
            "custom": false, "mode": null, "cloud": null, "warning": null,
            "last_scan": null, "files": 0, "errors": [],
        }),
    })
}

/// Folders of the system and of the person's keys, under the home folder (`HOME`, `USERPROFILE`) or from the root. A
/// space never sits in one, nor in a folder that holds one: in `read` mode the node would read the person's secrets into
/// a space that any of their assistants reads, and in `readwrite` mode it would write there.
const SECRET_HOME: [&str; 16] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".config",
    ".password-store",
    ".local/share/keyrings",
    ".netrc",
    ".pgpass",
    "Library/Keychains",
    "Library/Cookies",
    "Library/Application Support/com.apple.TCC",
    "AppData/Roaming/Microsoft/Credentials",
    "AppData/Local/Microsoft/Credentials",
];
const SECRET_ROOT: [&str; 12] = [
    "/etc",
    "/private/etc",
    "/private/var/db",
    "/System",
    "/Library/Keychains",
    "/usr",
    "/bin",
    "/sbin",
    "/boot",
    "/root",
    "/proc",
    "/sys",
];

/// `path` with its deepest existing ancestor resolved (links followed) and the rest, not made yet, appended as it is.
fn resolved_ahead(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(c) = fs::canonicalize(at) {
            return rest.iter().rev().fold(c, |p: PathBuf, part| p.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                at = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Refuses a folder that is, is inside, or holds a place of [`SECRET_HOME`] or [`SECRET_ROOT`] (compared after
/// resolving links, as `canon` is).
fn refuse_secret_place(canon: &Path) -> Result<()> {
    let homes = ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .filter(|h| h.is_absolute());
    let mut places: Vec<PathBuf> = SECRET_ROOT.iter().map(PathBuf::from).collect();
    for home in homes {
        places.extend(SECRET_HOME.iter().map(|p| home.join(p)));
    }
    for place in places {
        // Resolved the way `canon` is, even when the place does not exist on this machine: on Windows a resolved path
        // carries the `\\?\` prefix, and a place left as written would never compare equal to one.
        let place = resolved_ahead(&place);
        if canon.starts_with(&place) || place.starts_with(canon) {
            return Err(Error::Invalid(format!(
                "{} is, or holds, a folder of the system or of keys ({}): a space never sits there",
                canon.display(),
                place.display()
            )));
        }
    }
    Ok(())
}

/// Points a space at a folder of this node, or (`path` `None`) back at the default storage folder: the node then stops
/// mirroring and leaves the folder as it is. The folder is made if it does not exist. It must not hold, or be held by,
/// the node's own data directory or the folder of another space of this node. The library index of the space follows
/// the folder (one setting). Returns [`info`]. Nothing is read or written in the folder yet: see [`sync_space`].
///
/// # Errors
/// [`Error::Invalid`] for a relative path, a file, an overlap; [`Error::UnknownSpace`].
pub fn set(
    brain: &Brain,
    space: &Space,
    path: Option<&Path>,
    mode: Mode,
    now: i64,
) -> Result<Value> {
    if !brain.has_space(space)? {
        return Err(Error::UnknownSpace(space.to_string()));
    }
    let ctl = brain.open_control()?;
    let old = read_row(&ctl, space)?;
    let conn = brain.open_space(space)?;
    let default = default_path(brain, space);
    let Some(path) = path.filter(|p| *p != default.as_path()) else {
        if let Some(old) = old {
            let tx = rusqlite::Transaction::new_unchecked(
                &ctl,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            tx.execute(
                "DELETE FROM space_folders WHERE space = ?1",
                [space.as_str()],
            )?;
            tx.execute(
                "DELETE FROM folder_state WHERE space = ?1",
                [space.as_str()],
            )?;
            tx.commit()?;
            for r in library::roots(&conn)? {
                if Path::new(&r.root) == old.path {
                    library::remove_root(&conn, r.id)?;
                }
            }
        }
        return info(brain, space);
    };
    if !path.is_absolute() {
        return Err(Error::Invalid(
            "give the full path of the folder (it starts at the root of the disk)".into(),
        ));
    }
    if path.exists() && !path.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::Invalid("give the folder's path without `..`".into()));
    }
    // Checked before anything is made on the disk, from the deepest part of the path that exists, and again below once
    // the folder exists and every link in it is resolved.
    refuse_secret_place(&resolved_ahead(path))?;
    fs::create_dir_all(path).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    let canon =
        fs::canonicalize(path).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    if canon.parent().is_none() {
        return Err(Error::Invalid(
            "the root of the disk is not a folder for a space".into(),
        ));
    }
    refuse_secret_place(&canon)?;
    let data = fs::canonicalize(brain.dir())?;
    // Where the node writes, never its own data directory; a folder only read (a library) may be anywhere.
    if mode == Mode::ReadWrite && (canon.starts_with(&data) || data.starts_with(&canon)) {
        return Err(Error::Invalid(
            "the folder must be outside the node's own data directory (its databases must never sit in a folder others write)".into(),
        ));
    }
    for other in all(brain)? {
        if other.space != *space
            && (mode == Mode::ReadWrite || other.mode == Mode::ReadWrite)
            && (canon.starts_with(&other.path) || other.path.starts_with(&canon))
        {
            return Err(Error::Invalid(format!(
                "{} overlaps the folder of {} on this node",
                canon.display(),
                other.space
            )));
        }
    }
    let text = canon
        .to_str()
        .ok_or_else(|| Error::Invalid("the folder's path is not text".into()))?
        .to_string();
    let cloud = cloud_of(path).or_else(|| cloud_of(&canon));
    let same = old.as_ref().is_some_and(|o| o.path == canon);
    let tx = rusqlite::Transaction::new_unchecked(&ctl, rusqlite::TransactionBehavior::Immediate)?;
    if !same {
        tx.execute(
            "DELETE FROM folder_state WHERE space = ?1",
            [space.as_str()],
        )?;
    }
    tx.execute(
        "INSERT INTO space_folders (space, path, mode, cloud, set_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(space) DO UPDATE SET path = ?2, mode = ?3, cloud = ?4, set_at = ?5,
           last_scan = CASE WHEN ?6 THEN last_scan END, last_files = CASE WHEN ?6 THEN last_files ELSE 0 END,
           last_errors = CASE WHEN ?6 THEN last_errors ELSE '[]' END",
        params![space.as_str(), text, mode.as_str(), cloud, now, same],
    )?;
    tx.commit()?;
    // One setting: the search index of the space reads this folder, and only this one.
    for r in library::roots(&conn)? {
        if r.root != text {
            library::remove_root(&conn, r.id)?;
        }
    }
    library::add_root(&conn, &canon, now)?;
    info(brain, space)
}

// ---------------------------------------------------------------------------------------------------------------
// What the node remembers of each file of a folder.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Note,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    /// The file and the memory are in step (as of the last pass).
    Tracked,
    /// The memory retired it; the file was left where it is.
    Retired,
    /// The engine could not take it; `err` says why. Looked at again only when the file changes.
    Ignored,
}

#[derive(Clone, Debug)]
struct Row {
    rel: String,
    kind: Kind,
    obj: Option<String>,
    sha: String,
    mtime: i64,
    size: i64,
    st: St,
    err: Option<String>,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Note => "note",
            Kind::File => "file",
        }
    }
}

impl St {
    fn as_str(self) -> &'static str {
        match self {
            St::Tracked => "tracked",
            St::Retired => "retired",
            St::Ignored => "ignored",
        }
    }
}

fn load_rows(ctl: &Connection, space: &Space) -> Result<BTreeMap<String, Row>> {
    let mut st = ctl.prepare(
        "SELECT rel, kind, obj, sha, mtime, size, state, err FROM folder_state WHERE space = ?1",
    )?;
    let rows = st.query_map([space.as_str()], |r| {
        let kind: String = r.get(1)?;
        let state: String = r.get(6)?;
        Ok(Row {
            rel: r.get(0)?,
            kind: if kind == "file" {
                Kind::File
            } else {
                Kind::Note
            },
            obj: r.get(2)?,
            sha: r.get(3)?,
            mtime: r.get(4)?,
            size: r.get(5)?,
            st: match state.as_str() {
                "retired" => St::Retired,
                "ignored" => St::Ignored,
                _ => St::Tracked,
            },
            err: r.get(7)?,
        })
    })?;
    let mut out = BTreeMap::new();
    for r in rows {
        let r = r?;
        out.insert(r.rel.clone(), r);
    }
    Ok(out)
}

fn put_row(ctl: &Connection, space: &Space, r: &Row) -> Result<()> {
    ctl.execute(
        "INSERT OR REPLACE INTO folder_state (space, rel, kind, obj, sha, mtime, size, state, err)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            space.as_str(),
            r.rel,
            r.kind.as_str(),
            r.obj,
            r.sha,
            r.mtime,
            r.size,
            r.st.as_str(),
            r.err
        ],
    )?;
    Ok(())
}

fn del_row(ctl: &Connection, space: &Space, rel: &str) -> Result<()> {
    ctl.execute(
        "DELETE FROM folder_state WHERE space = ?1 AND rel = ?2",
        params![space.as_str(), rel],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// The folder on disk.

#[derive(Clone, Debug)]
struct Seen {
    size: i64,
    mtime: i64,
}

fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

fn skipped(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with('~')
        || name
            .rsplit_once('.')
            .is_some_and(|(_, e)| UNFINISHED.contains(&e.to_lowercase().as_str()))
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut BTreeMap<String, Seen>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if skipped(&name) || out.len() >= MAX_ENTRIES {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let here = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if ft.is_dir() {
            walk(&entry.path(), &here, depth + 1, out);
        } else if ft.is_file()
            && let Ok(meta) = entry.metadata()
        {
            out.insert(
                here,
                Seen {
                    size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
                    mtime: mtime_ns(&meta),
                },
            );
        }
    }
}

/// Writes a file next to its final place then renames it: a program that watches the folder never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<Seen> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Invalid("no folder".into()))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.memcp-tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    let meta = fs::metadata(path)?;
    Ok(Seen {
        size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
        mtime: mtime_ns(&meta),
    })
}

/// [`write_atomic`] from a file, copied a block at a time.
fn copy_atomic(from: &Path, path: &Path) -> Result<Seen> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Invalid("no folder".into()))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.memcp-tmp"));
    fs::copy(from, &tmp)?;
    fs::File::open(&tmp)?.sync_all()?;
    fs::rename(&tmp, path)?;
    let meta = fs::metadata(path)?;
    Ok(Seen {
        size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
        mtime: mtime_ns(&meta),
    })
}

fn within(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

// ---------------------------------------------------------------------------------------------------------------
// The memory side.

/// The newest version of a note: follows `replaced_by` to the end. `None` if the id is unknown.
fn head_of(conn: &Connection, id: &str) -> Result<Option<Fact>> {
    let Some(mut cur) = facts::read_fact(conn, id)? else {
        return Ok(None);
    };
    for _ in 0..100_000 {
        match cur.replaced_by.clone() {
            Some(next) if cur.status == "replaced" => match facts::read_fact(conn, &next)? {
                Some(n) => cur = n,
                None => break,
            },
            _ => break,
        }
    }
    Ok(Some(cur))
}

/// Notes of the space the folder shows: active, not the node's own bookkeeping (files' descriptions, rules, boards).
fn mirrored_note_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT id FROM facts WHERE status = 'active' AND type IS NULL
         AND id NOT IN (SELECT fact_id FROM fact_tags WHERE tag IN ('dashboard','rules','index','guide')
                        OR tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
         ORDER BY created_at, id",
    )?;
    let ids = st.query_map([], |r| r.get::<_, String>(0))?;
    Ok(ids.collect::<std::result::Result<_, _>>()?)
}

fn render_for(conn: &Connection, f: &Fact) -> Result<String> {
    let chain = facts::history(conn, &f.id)?;
    let created = chain.first().map_or(f.created_at, |c| c.created_at);
    Ok(render_note(f, created, chain.len().max(1)))
}

/// Writes a note into the memory as a note of its own, whatever is close to it.
fn remember_note(
    conn: &Connection,
    space: &Space,
    text: &str,
    tags: Vec<String>,
) -> Result<Remembered> {
    let mut fact = NewFact::agent(text, &format!("agent:{AUTHOR}"));
    fact.tags = tags;
    let mut decided: Vec<String> = vec![];
    loop {
        match facts::remember(conn, space, AUTHOR, fact.clone(), &decided)? {
            Remembered::NeedsDecision(c) if decided.is_empty() => {
                decided = c.into_iter().map(|c| c.id).collect();
            }
            Remembered::NeedsDecision(_) => {
                return Err(Error::Invalid("close notes already exist".into()));
            }
            other => return Ok(other),
        }
    }
}

fn basename(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

fn is_note_path(rel: &str) -> bool {
    let lower = rel.to_lowercase();
    !rel.starts_with(&format!("{FILES_DIR}/"))
        && (lower.ends_with(".md") || lower.ends_with(".markdown"))
}

fn short_err(e: &dyn std::fmt::Display) -> String {
    e.to_string().chars().take(200).collect()
}

/// The lock of a pass: one pass at a time on a node, across the processes that share its data directory.
fn lock(brain: &Brain) -> Result<fs::File> {
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(brain.dir().join("folder-sync.lock"))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(fs::TryLockError::WouldBlock) => Err(Error::Invalid(
            "a folder pass is already running on this node".into(),
        )),
        Err(fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

struct Pass<'a> {
    brain: &'a Brain,
    space: &'a Space,
    conn: Connection,
    ctl: Connection,
    root: PathBuf,
    rows: BTreeMap<String, Row>,
    scan: BTreeMap<String, Seen>,
    now: i64,
    opts: Options,
    rep: Report,
}

impl Pass<'_> {
    fn settled(&self, seen: &Seen) -> bool {
        seen.mtime / 1_000_000_000 + self.opts.settle_secs <= self.now
    }

    fn save(&mut self, row: Row) -> Result<()> {
        put_row(&self.ctl, self.space, &row)?;
        self.rows.insert(row.rel.clone(), row);
        Ok(())
    }

    fn drop_row(&mut self, rel: &str) -> Result<()> {
        del_row(&self.ctl, self.space, rel)?;
        self.rows.remove(rel);
        Ok(())
    }

    /// Reads a file; `None` when it cannot be read now (a cloud placeholder not downloaded, a file being replaced).
    fn read(&mut self, rel: &str) -> Option<Vec<u8>> {
        let path = within(&self.root, rel);
        if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE as u64) {
            self.rep.errors.push(format!(
                "{rel}: too big (over {} MB), left alone",
                MAX_FILE / 1024 / 1024
            ));
            return None;
        }
        match fs::read(path) {
            Ok(b) => Some(b),
            Err(e) => {
                self.rep
                    .errors
                    .push(format!("{rel}: cannot be read now ({})", short_err(&e)));
                None
            }
        }
    }

    /// Writes the current text of a note over its tracked file and records what was written.
    fn write_note(&mut self, rel: &str, head: &Fact) -> Result<()> {
        let body = render_for(&self.conn, head)?;
        let seen = write_atomic(&within(&self.root, rel), body.as_bytes())?;
        self.save(Row {
            rel: rel.to_string(),
            kind: Kind::Note,
            obj: Some(head.id.clone()),
            sha: hash_bytes(body.as_bytes()),
            mtime: seen.mtime,
            size: seen.size,
            st: St::Tracked,
            err: None,
        })?;
        self.rep.written += 1;
        Ok(())
    }

    fn ignore(
        &mut self,
        rel: &str,
        kind: Kind,
        sha: String,
        seen: &Seen,
        why: String,
    ) -> Result<()> {
        self.rep.errors.push(format!("{rel}: {why}"));
        self.save(Row {
            rel: rel.to_string(),
            kind,
            obj: None,
            sha,
            mtime: seen.mtime,
            size: seen.size,
            st: St::Ignored,
            err: Some(why),
        })
    }

    /// A tracked file that is present.
    fn tracked_present(&mut self, row: Row) -> Result<()> {
        let Some(seen) = self.scan.get(&row.rel).cloned() else {
            return Ok(());
        };
        let rel = row.rel.clone();
        // Unchanged by size and time: not even read.
        let mut bytes = None;
        let mut sha = row.sha.clone();
        if seen.size != row.size || seen.mtime != row.mtime {
            let Some(b) = self.read(&rel) else {
                return Ok(());
            };
            sha = hash_bytes(&b);
            if sha == row.sha {
                // Touched, not changed (a sync tool, a copy back): what the node wrote is never read as an edit.
                self.save(Row {
                    mtime: seen.mtime,
                    size: seen.size,
                    ..row.clone()
                })?;
            } else if self.settled(&seen) {
                bytes = Some(b);
            } else {
                self.rep.waiting += 1;
                return Ok(());
            }
        }
        let obj = row.obj.clone().unwrap_or_default();
        match row.kind {
            Kind::File => self.tracked_file(row, &seen, bytes, sha),
            Kind::Note => {
                let head = head_of(&self.conn, &obj)?;
                self.tracked_note(row, &seen, bytes, sha, head)
            }
        }
    }

    fn tracked_file(
        &mut self,
        row: Row,
        seen: &Seen,
        bytes: Option<Vec<u8>>,
        sha: String,
    ) -> Result<()> {
        let obj = row.obj.clone().unwrap_or_default();
        let info = files::info(self.brain, self.space, &obj)?;
        let retired = info.as_ref().is_none_or(|i| i.status != "active");
        match bytes {
            None if retired => {
                self.save(Row {
                    st: St::Retired,
                    ..row
                })?;
            }
            None => {}
            Some(b) => {
                // The file was edited in the folder: its new content is a file of the space (the old one is retired).
                match files::put(self.brain, self.space, AUTHOR, basename(&row.rel), "", &b) {
                    Ok(new) => {
                        if !retired {
                            let _ = files::retire(
                                self.brain,
                                self.space,
                                AUTHOR,
                                &obj,
                                "replaced by an edit in the folder",
                            );
                        }
                        self.rep.edited += 1;
                        self.save(Row {
                            obj: Some(new.id),
                            sha,
                            mtime: seen.mtime,
                            size: seen.size,
                            st: St::Tracked,
                            err: None,
                            ..row
                        })?;
                    }
                    Err(e) => self.ignore(&row.rel, Kind::File, sha, seen, short_err(&e))?,
                }
            }
        }
        Ok(())
    }

    fn tracked_note(
        &mut self,
        row: Row,
        seen: &Seen,
        bytes: Option<Vec<u8>>,
        sha: String,
        head: Option<Fact>,
    ) -> Result<()> {
        let obj = row.obj.clone().unwrap_or_default();
        let rel = row.rel.clone();
        let Some(head) = head else {
            // The note is not in this memory any more (never expected): leave the file, say so.
            return self.ignore(
                &rel,
                Kind::Note,
                sha,
                seen,
                "its note is not in the memory".into(),
            );
        };
        let active = head.status == "active";
        let newer = active && head.id != obj;
        let Some(b) = bytes else {
            if !active {
                self.save(Row {
                    st: St::Retired,
                    ..row
                })?;
            } else if newer {
                self.write_note(&rel, &head)?;
            }
            return Ok(());
        };
        if !active {
            // Retired in the memory, edited in the folder: the edit is a new note.
            self.drop_row(&rel)?;
            return self.create_from_file(&rel, &b, seen);
        }
        let p = parse_note(&String::from_utf8_lossy(&b));
        let text = text::normalize(&p.body);
        let tags = match p.tags.as_deref().map(facts::normalize_tags).transpose() {
            Ok(t) => t,
            Err(e) => return self.ignore(&rel, Kind::Note, sha, seen, short_err(&e)),
        };
        if text.is_empty() {
            return self.ignore(
                &rel,
                Kind::Note,
                sha,
                seen,
                "the note is empty: delete the file to retire the note".into(),
            );
        }
        let same = text::text_hash(&text) == text::text_hash(&head.text)
            && tags
                .as_ref()
                .is_none_or(|t| sorted(t) == sorted(&head.tags));
        let adopt = |me: &mut Self, id: String, err: Option<String>| {
            me.save(Row {
                obj: Some(id),
                sha: sha.clone(),
                mtime: seen.mtime,
                size: seen.size,
                st: St::Tracked,
                err,
                ..row.clone()
            })
        };
        if same {
            // A change that says what the memory already says (reformatting, an old id): nothing to apply.
            adopt(self, head.id.clone(), None)
        } else if newer {
            // Both sides changed: the memory's version stays, the folder's copy is kept as a note of its own.
            let mut ctags = tags.unwrap_or_else(|| head.tags.clone());
            ctags.push(CONFLICT_TAG.into());
            match remember_note(
                &self.conn,
                self.space,
                &text,
                facts::normalize_tags(&ctags)?,
            ) {
                Ok(_) => self.rep.conflicts += 1,
                Err(e) => self
                    .rep
                    .errors
                    .push(format!("{rel}: conflict not kept ({})", short_err(&e))),
            }
            self.write_note(&rel, &head)
        } else {
            let mut new = NewFact::agent(&text, &format!("agent:{AUTHOR}"));
            new.kind = head.kind.clone();
            new.valid_from = Some(head.valid_from);
            new.valid_to = head.valid_to;
            new.tags = tags.unwrap_or_default();
            match facts::supersede(
                &self.conn,
                self.space,
                AUTHOR,
                &head.id,
                new,
                Some("edited in the folder on this node"),
            ) {
                Ok(f) => {
                    self.rep.edited += 1;
                    adopt(self, f.id, None)
                }
                Err(e) => {
                    // Looked at again only when the file changes: the same text is not tried every pass.
                    let why = short_err(&e);
                    self.rep.errors.push(format!("{rel}: {why}"));
                    adopt(self, head.id.clone(), Some(why))
                }
            }
        }
    }

    /// A file of the folder the node did not know: a note (`.md` outside `files/`) or a file of the space.
    fn create_from_file(&mut self, rel: &str, bytes: &[u8], seen: &Seen) -> Result<()> {
        let sha = hash_bytes(bytes);
        let mut row = Row {
            rel: rel.to_string(),
            kind: Kind::Note,
            obj: None,
            sha: sha.clone(),
            mtime: seen.mtime,
            size: seen.size,
            st: St::Tracked,
            err: None,
        };
        if is_note_path(rel) {
            let p = parse_note(&String::from_utf8_lossy(bytes));
            let text = text::normalize(&p.body);
            if text.is_empty() {
                return self.ignore(rel, Kind::Note, sha, seen, "the note is empty".into());
            }
            let tags = match facts::normalize_tags(&p.tags.unwrap_or_default()) {
                Ok(t) => t,
                Err(e) => return self.ignore(rel, Kind::Note, sha, seen, short_err(&e)),
            };
            match remember_note(&self.conn, self.space, &text, tags) {
                Ok(Remembered::Created(f)) => {
                    self.rep.new_notes += 1;
                    row.obj = Some(f.id);
                    self.save(row)
                }
                // The note is already in the memory (this file is its copy, from before the node knew it): adopted,
                // unless another file already stands for it.
                Ok(Remembered::Duplicate(f)) => {
                    let taken = self.rows.values().any(|r| {
                        r.kind == Kind::Note
                            && r.st == St::Tracked
                            && r.obj.as_deref() == Some(f.id.as_str())
                    });
                    if taken {
                        let why = format!("same text as the note {} (another file)", f.id);
                        self.ignore(rel, Kind::Note, sha, seen, why)
                    } else {
                        row.obj = Some(f.id);
                        self.save(row)
                    }
                }
                Ok(Remembered::NeedsDecision(_)) => Ok(()),
                Err(e) => self.ignore(rel, Kind::Note, sha, seen, short_err(&e)),
            }
        } else {
            row.kind = Kind::File;
            match files::put(self.brain, self.space, AUTHOR, basename(rel), "", bytes) {
                Ok(f) => {
                    self.rep.new_files += 1;
                    row.obj = Some(f.id);
                    self.save(row)
                }
                Err(e) => self.ignore(rel, Kind::File, sha, seen, short_err(&e)),
            }
        }
    }

    /// The files of the folder the node did not know (or set aside as ignored or retired and that changed since).
    fn untracked(&mut self, rels: Vec<String>) -> Result<()> {
        for rel in rels {
            let Some(seen) = self.scan.get(&rel).cloned() else {
                continue;
            };
            if self
                .rows
                .get(&rel)
                .is_some_and(|r| r.size == seen.size && r.mtime == seen.mtime)
            {
                continue;
            }
            if seen.size > MAX_FILE {
                self.ignore(
                    &rel,
                    Kind::File,
                    String::new(),
                    &seen,
                    format!("too big (over {} MB)", MAX_FILE / 1024 / 1024),
                )?;
                continue;
            }
            if !self.settled(&seen) {
                self.rep.waiting += 1;
                continue;
            }
            let Some(bytes) = self.read(&rel) else {
                continue;
            };
            if let Some(r) = self.rows.get(&rel).cloned()
                && r.sha == hash_bytes(&bytes)
            {
                // Same content as the one set aside: only the date moved.
                self.save(Row {
                    mtime: seen.mtime,
                    size: seen.size,
                    ..r
                })?;
                continue;
            }
            self.create_from_file(&rel, &bytes, &seen)?;
        }
        Ok(())
    }

    /// Tracked files that are gone: renamed (found again under another name) or deleted.
    fn vanished(&mut self, gone: Vec<Row>, fresh: &mut Vec<String>) -> Result<()> {
        let tracked_total = self.rows.values().filter(|r| r.st == St::Tracked).count();
        if gone.len() >= 5 && gone.len() * 2 > tracked_total {
            self.rep.errors.push(format!(
                "{} of {tracked_total} files are gone at once: the folder looks unmounted or being synced, nothing was retired",
                gone.len(),
            ));
            return Ok(());
        }
        // A rename: a file the node did not know that is one of the gone ones (same content, or the front matter's id).
        let mut gone = gone;
        let mut moved: Vec<String> = vec![];
        for new in fresh.clone() {
            let path = within(&self.root, &new);
            if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE as u64) {
                continue;
            }
            let Ok(bytes) = fs::read(path) else {
                continue;
            };
            let sha = hash_bytes(&bytes);
            let note = is_note_path(&new);
            let fm_id = if note {
                parse_note(&String::from_utf8_lossy(&bytes)).id
            } else {
                None
            };
            let pos = gone.iter().position(|g| match g.kind {
                Kind::File => !note && g.sha == sha,
                Kind::Note => {
                    note && (g.sha == sha
                        || fm_id.as_deref().is_some_and(|id| {
                            g.obj.as_deref().is_some_and(|o| {
                                o == id
                                    || facts::history(&self.conn, o)
                                        .is_ok_and(|h| h.iter().any(|f| f.id == id))
                            })
                        }))
                }
            });
            if let Some(i) = pos {
                let g = gone.remove(i);
                self.drop_row(&g.rel)?;
                // Size and time unknown on purpose: the file is read again as a tracked one under its new name.
                self.save(Row {
                    rel: new.clone(),
                    size: -1,
                    mtime: -1,
                    ..g
                })?;
                moved.push(new);
            }
        }
        fresh.retain(|f| !moved.contains(f));
        for rel in moved {
            if let Some(r) = self.rows.get(&rel).cloned() {
                self.tracked_present(r)?;
            }
        }
        for g in gone {
            let obj = g.obj.clone().unwrap_or_default();
            match (g.st, g.kind) {
                (St::Tracked, Kind::Note) => match head_of(&self.conn, &obj)? {
                    Some(h) if h.status == "active" && h.id == obj => {
                        facts::retire(
                            &self.conn,
                            AUTHOR,
                            &h.id,
                            "its file was deleted from the folder on this node",
                        )?;
                        self.rep.retired += 1;
                        self.drop_row(&g.rel)?;
                    }
                    // Changed in the memory since: the deletion does not win, the file comes back.
                    Some(h) if h.status == "active" => self.write_note(&g.rel, &h)?,
                    _ => self.drop_row(&g.rel)?,
                },
                (St::Tracked, Kind::File) => {
                    if files::info(self.brain, self.space, &obj)?
                        .is_some_and(|i| i.status == "active")
                    {
                        files::retire(
                            self.brain,
                            self.space,
                            AUTHOR,
                            &obj,
                            "its file was deleted from the folder on this node",
                        )?;
                        self.rep.retired += 1;
                    }
                    self.drop_row(&g.rel)?;
                }
                _ => self.drop_row(&g.rel)?,
            }
        }
        Ok(())
    }

    /// What the memory has that the folder does not show yet.
    fn mirror_out(&mut self) -> Result<()> {
        let mut shown: BTreeSet<String> = BTreeSet::new();
        for r in self.rows.values().filter(|r| r.st != St::Ignored) {
            let Some(o) = &r.obj else { continue };
            shown.insert(o.clone());
            if r.kind == Kind::Note
                && let Some(h) = head_of(&self.conn, o)?
            {
                shown.insert(h.id);
            }
        }
        let taken = |me: &Self, rel: &str| {
            me.rows.contains_key(rel) || me.scan.contains_key(rel) || within(&me.root, rel).exists()
        };
        for id in mirrored_note_ids(&self.conn)? {
            if shown.contains(&id) {
                continue;
            }
            let Some(f) = facts::read_fact(&self.conn, &id)? else {
                continue;
            };
            let base = format!("{}-{}", slug(&f.text), short_id(&id));
            let mut rel = format!("{base}.md");
            let mut n = 2;
            while taken(self, &rel) {
                rel = format!("{base}-{n}.md");
                n += 1;
            }
            self.write_note(&rel, &f)?;
        }
        let mut st = self.conn.prepare(
            "SELECT id, name, sha256 FROM files WHERE status = 'active' ORDER BY created_at, id",
        )?;
        let list: Vec<(String, String, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        drop(st);
        for (id, name, sha) in list {
            if shown.contains(&id) {
                continue;
            }
            let blob = files::blob_path(self.brain, self.space, &sha);
            let Ok(meta) = fs::metadata(&blob) else {
                continue;
            };
            // A big file is copied a block at a time; a smaller one is read and checked as before.
            let bytes = if meta.len() > MAX_FILE as u64 {
                None
            } else {
                let Some(b) = files::read_blob(self.brain, self.space, &sha)? else {
                    continue;
                };
                Some(b)
            };
            let name = if name.is_empty() || skipped(&name) {
                format!("file-{}", short_id(&id))
            } else {
                name
            };
            let mut rel = format!("{FILES_DIR}/{name}");
            if taken(self, &rel) {
                let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
                let tag = sha.get(..8).unwrap_or(&sha);
                let make = |n: usize| {
                    let n = if n == 1 {
                        String::new()
                    } else {
                        format!("-{n}")
                    };
                    if ext.is_empty() {
                        format!("{FILES_DIR}/{stem}-{tag}{n}")
                    } else {
                        format!("{FILES_DIR}/{stem}-{tag}{n}.{ext}")
                    }
                };
                let mut n = 1;
                rel = make(n);
                while taken(self, &rel) {
                    n += 1;
                    rel = make(n);
                }
            }
            let seen = match &bytes {
                Some(b) => write_atomic(&within(&self.root, &rel), b)?,
                None => copy_atomic(&blob, &within(&self.root, &rel))?,
            };
            self.save(Row {
                rel,
                kind: Kind::File,
                obj: Some(id),
                sha: bytes.as_deref().map_or(sha, hash_bytes),
                mtime: seen.mtime,
                size: seen.size,
                st: St::Tracked,
                err: None,
            })?;
            self.rep.written += 1;
        }
        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        // The files the node tracks: present ones are compared, absent ones are gone (renamed or deleted).
        let tracked: Vec<Row> = self
            .rows
            .values()
            .filter(|r| r.st == St::Tracked)
            .cloned()
            .collect();
        let mut gone = vec![];
        for r in tracked {
            if self.scan.contains_key(&r.rel) {
                self.tracked_present(r)?;
            } else {
                gone.push(r);
            }
        }
        let stale: Vec<String> = self
            .rows
            .values()
            .filter(|r| r.st != St::Tracked && !self.scan.contains_key(&r.rel))
            .map(|r| r.rel.clone())
            .collect();
        for rel in stale {
            self.drop_row(&rel)?;
        }
        let mut fresh: Vec<String> = self
            .scan
            .keys()
            .filter(|rel| self.rows.get(*rel).is_none_or(|r| r.st != St::Tracked))
            .cloned()
            .collect();
        // Deleted files retire their notes first: a renamed file (same text) must not meet its old note as a duplicate.
        self.vanished(gone, &mut fresh)?;
        self.untracked(fresh)?;
        self.mirror_out()
    }
}

/// One pass over the folder of a space: the memory is shown in the folder, what changed in the folder comes back.
/// Then the space's library index is refreshed. In `read` mode only the index is refreshed.
///
/// # Errors
/// [`Error::Invalid`] when no folder is set, when the folder is not reachable (nothing is changed), or when another
/// pass is running on this node.
pub fn sync_space(brain: &Brain, space: &Space, now: i64, opts: Options) -> Result<Report> {
    let ctl = brain.open_control()?;
    let folder = read_row(&ctl, space)?
        .ok_or_else(|| Error::Invalid(format!("{space} has no folder on this node")))?;
    let _lock = lock(brain)?;
    let conn = brain.open_space(space)?;
    let mut rep = Report::default();
    if !folder.path.is_dir() {
        let why = format!(
            "{} is not reachable: nothing was changed",
            folder.path.display()
        );
        ctl.execute(
            "UPDATE space_folders SET last_scan = ?2, last_errors = ?3 WHERE space = ?1",
            params![space.as_str(), now, json!([why]).to_string()],
        )?;
        return Err(Error::Invalid(why));
    }
    if folder.mode == Mode::ReadWrite {
        let mut scan = BTreeMap::new();
        walk(&folder.path, "", 0, &mut scan);
        let mut p = Pass {
            brain,
            space,
            conn: brain.open_space(space)?,
            ctl: brain.open_control()?,
            root: folder.path.clone(),
            rows: load_rows(&ctl, space)?,
            scan,
            now,
            opts,
            rep: Report::default(),
        };
        p.run()?;
        rep = p.rep;
    }
    // One setting: the library index reads the same folder, so that the library tools find what is in it.
    if let Ok(id) = library::add_root(&conn, &folder.path, now)
        && let Err(e) = library::rescan(&conn, id, now)
    {
        rep.errors.push(format!("index: {}", short_err(&e)));
    }
    let rows = load_rows(&ctl, space)?;
    let mut errors: Vec<String> = rows
        .values()
        .filter_map(|r| r.err.as_ref().map(|e| format!("{}: {e}", r.rel)))
        .collect();
    for e in &rep.errors {
        if !errors.contains(e) {
            errors.push(e.clone());
        }
    }
    errors.truncate(MAX_ERRORS);
    let tracked = rows.values().filter(|r| r.st == St::Tracked).count();
    ctl.execute(
        "UPDATE space_folders SET last_scan = ?2, last_files = ?3, last_errors = ?4 WHERE space = ?1",
        params![
            space.as_str(),
            now,
            i64::try_from(tracked).unwrap_or(0),
            serde_json::to_string(&errors).unwrap_or_else(|_| "[]".into())
        ],
    )?;
    Ok(rep)
}

/// One pass for every space that has a folder on this node. A space whose pass fails is reported, the others go on.
pub fn sync_all(brain: &Brain, now: i64, opts: Options) -> Vec<(Space, Result<Report>)> {
    let Ok(list) = all(brain) else {
        return vec![];
    };
    list.into_iter()
        .map(|f| {
            let r = sync_space(brain, &f.space, now, opts);
            (f.space, r)
        })
        .collect()
}
