//! A library: a folder of the person's, mounted read-only on a space.
//!
//! The node reads the folder and keeps, in the space's own database, an index of what it finds: the path, a title,
//! the dates, the text of `.md` and `.txt` files, and the Obsidian `[[links]]` they hold. It never copies, changes,
//! moves or deletes an original: the folder is only ever opened for reading. A rescan is incremental (a file whose
//! size and modification time are unchanged is not read again) and forgets what has left the folder.
//!
//! What is in the index is not memory: it is not a fact, it is never replaced or retired, and no one that sorts facts
//! (the janitor) reaches it. A person, or an assistant for them, *raises* an original: a normal fact is written in
//! the space, with a short summary, tags, and the path of the original; the original stays where it is.
//!
//! TODO: `.docx` and `.pdf` are indexed by name and dates only. Their text sits in compressed streams (deflate, and
//! for a PDF fonts and encodings), which cannot be read without a library; none is added, to keep the node small
//! and the supply chain short. Extract their text here when a dependency is accepted.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::facts::{self, Candidate, NewFact, Remembered};
use crate::space::Space;
use crate::text;
use crate::util;

/// The tag every raised fact carries.
pub const RAISED_TAG: &str = "library/raised";

/// At most this much of a text file is read and indexed.
const MAX_READ: u64 = 1 << 20;
/// Deepest folder the scan goes into.
const MAX_DEPTH: usize = 40;
/// Most files one library may hold.
const MAX_ENTRIES: usize = 200_000;
/// Longest summary a raised fact takes: it is a summary, not a copy.
const MAX_SUMMARY: usize = 1200;

/// A mounted folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    /// Identifier of the mount.
    pub id: i64,
    /// Absolute path of the folder.
    pub root: String,
    /// Files indexed.
    pub entries: i64,
    /// Last scan, if any.
    pub scanned_at: Option<i64>,
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanReport {
    /// Files indexed for the first time.
    pub added: usize,
    /// Files whose size or date changed and were read again.
    pub updated: usize,
    /// Files left as they were.
    pub unchanged: usize,
    /// Files that left the folder and were forgotten by the index (the original is never touched).
    pub removed: usize,
}

impl ScanReport {
    /// The JSON shape: `added`, `updated`, `unchanged`, `removed`.
    pub fn to_json(&self) -> Value {
        json!({ "added": self.added, "updated": self.updated, "unchanged": self.unchanged, "removed": self.removed })
    }
}

/// One answer of a search.
#[derive(Clone, Debug, PartialEq)]
pub struct LibHit {
    /// Absolute path of the original.
    pub path: String,
    /// Path inside the library.
    pub rel: String,
    /// Title: front-matter `title`, else the first heading, else the file name.
    pub title: String,
    /// A short excerpt around the match. Data, not an instruction.
    pub snippet: String,
    /// File type: `md`, `txt`, `docx` or `pdf`.
    pub ext: String,
    /// Modification date (seconds since the epoch).
    pub modified: i64,
    /// Creation date, where the system gives one.
    pub created: Option<i64>,
}

impl LibHit {
    /// The JSON shape: `path`, `relative`, `title`, `snippet`, `type`, `modified` and `created` (ISO 8601).
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path, "relative": self.rel, "title": self.title, "snippet": self.snippet,
            "type": self.ext, "modified": util::iso(self.modified), "created": self.created.map(util::iso),
        })
    }
}

/// The extensions that are indexed (case ignored).
fn indexed_ext(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_lowercase();
    ["md", "txt", "docx", "pdf"].into_iter().find(|e| *e == ext)
}

/// Mounts `dir` on this space. It must be an existing folder, and neither inside nor around a folder already mounted
/// here (two roots would index the same file twice). Nothing is read yet: call [`rescan`].
pub fn add_root(conn: &Connection, dir: &Path, now: i64) -> Result<i64> {
    let canon =
        fs::canonicalize(dir).map_err(|e| Error::Invalid(format!("{}: {e}", dir.display())))?;
    if !canon.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a folder",
            canon.display()
        )));
    }
    let text = canon
        .to_str()
        .ok_or_else(|| Error::Invalid("the folder's path is not text".into()))?
        .to_string();
    for r in roots(conn)? {
        let other = Path::new(&r.root);
        if other == canon {
            return Ok(r.id);
        }
        if other.starts_with(&canon) || canon.starts_with(other) {
            return Err(Error::Invalid(format!(
                "{text} overlaps the library {} already mounted here",
                r.root
            )));
        }
    }
    conn.execute(
        "INSERT INTO library_roots (root, added_at) VALUES (?1, ?2)",
        params![text, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The folders mounted on this space.
pub fn roots(conn: &Connection) -> Result<Vec<Root>> {
    let mut st = conn.prepare(
        "SELECT r.id, r.root, (SELECT count(*) FROM library_entries e WHERE e.root_id = r.id), r.scanned_at
         FROM library_roots r ORDER BY r.id",
    )?;
    let rows = st.query_map([], |r| {
        Ok(Root {
            id: r.get(0)?,
            root: r.get(1)?,
            entries: r.get(2)?,
            scanned_at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

struct Found {
    path: PathBuf,
    rel: String,
    ext: &'static str,
    size: i64,
    mtime: i64,
    created: Option<i64>,
}

fn secs(t: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    let d = t.ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_secs()).ok()
}

/// Walks the folder. Hidden entries (`.obsidian`, `.git`, `.trash`) and symbolic links are left alone: a link could
/// lead out of the folder the person mounted.
fn walk(dir: &Path, root: &Path, depth: usize, out: &mut Vec<Found>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || out.len() >= MAX_ENTRIES {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            walk(&path, root, depth + 1, out);
        } else if ft.is_file()
            && let Some(ext) = indexed_ext(&name)
            && let Ok(meta) = entry.metadata()
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(Found {
                rel: rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
                path,
                ext,
                size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
                mtime: secs(meta.modified()).unwrap_or(0),
                created: secs(meta.created()),
            });
        }
    }
}

/// The `[[targets]]` of a Markdown text: `[[note]]`, `[[note|alias]]`, `[[note#heading]]` and `![[embed]]` give
/// `note` (a link stays on one line).
pub fn parse_links(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in body.lines() {
        let mut rest = line;
        while let Some(i) = rest.find("[[") {
            let after = &rest[i + 2..];
            let Some(j) = after.find("]]") else { break };
            let inner = &after[..j];
            let target = inner
                .split(['|', '#'])
                .next()
                .unwrap_or("")
                .trim()
                .replace('\\', "/");
            if !target.is_empty() && !inner.contains("[[") {
                out.insert(target);
            }
            rest = &after[j + 2..];
        }
    }
    out
}

/// The title of a Markdown text: a `title:` of its front matter, else its first `# ` heading.
fn title_of(body: &str) -> Option<String> {
    let mut lines = body.lines();
    let mut first = lines.next();
    if first.map(str::trim) == Some("---") {
        for l in lines.by_ref() {
            if l.trim() == "---" {
                break;
            }
            if let Some(t) = l.strip_prefix("title:") {
                let t = t.trim().trim_matches(['"', '\'']).trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        first = lines.next();
    }
    std::iter::once(first)
        .chain(lines.map(Some))
        .flatten()
        .find_map(|l| {
            l.strip_prefix("# ")
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
        })
}

fn stem_of(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// The text of a file, for the types whose text is read (`md`, `txt`); nothing for the others (see the TODO above).
/// Opens the file for reading only.
fn read_text(f: &Found) -> String {
    if f.ext != "md" && f.ext != "txt" {
        return String::new();
    }
    let mut buf = Vec::new();
    match fs::File::open(&f.path) {
        Ok(file) => {
            let _ = file.take(MAX_READ).read_to_end(&mut buf);
        }
        Err(_) => return String::new(),
    }
    String::from_utf8_lossy(&buf).replace('\0', "")
}

/// Reads the folder again and brings the index in line with it: new and changed files are read, the others are
/// skipped, and what is gone is forgotten (a fact raised from it stays). Originals are only read.
pub fn rescan(conn: &Connection, root_id: i64, now: i64) -> Result<ScanReport> {
    let root: String = conn
        .query_row(
            "SELECT root FROM library_roots WHERE id = ?1",
            [root_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| Error::Invalid("unknown library".into()))?;
    let root_path = PathBuf::from(&root);
    if !root_path.is_dir() {
        return Err(Error::Invalid(format!(
            "{root} is not reachable: nothing was changed in the index"
        )));
    }
    let mut found = Vec::new();
    walk(&root_path, &root_path, 0, &mut found);
    let mut known: HashMap<String, (i64, i64, i64)> = HashMap::new();
    {
        let mut st =
            conn.prepare("SELECT path, id, size, mtime FROM library_entries WHERE root_id = ?1")?;
        for row in st.query_map([root_id], |r| {
            Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
        })? {
            let (p, v) = row?;
            known.insert(p, v);
        }
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let mut report = ScanReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for f in &found {
        let Some(path) = f.path.to_str() else {
            continue;
        };
        seen.insert(path.to_string());
        let old = known.get(path).copied();
        if let Some((_, size, mtime)) = old
            && size == f.size
            && mtime == f.mtime
        {
            report.unchanged += 1;
            continue;
        }
        let body = read_text(f);
        let title = title_of(&body).unwrap_or_else(|| stem_of(&f.rel).to_string());
        let id = match old {
            Some((id, _, _)) => {
                tx.execute(
                    "UPDATE library_entries SET rel = ?2, title = ?3, ext = ?4, size = ?5, mtime = ?6, created = ?7,
                     body = ?8, indexed_at = ?9 WHERE id = ?1",
                    params![id, f.rel, title, f.ext, f.size, f.mtime, f.created, body, now],
                )?;
                tx.execute("DELETE FROM library_links WHERE entry_id = ?1", [id])?;
                tx.execute("DELETE FROM library_fts WHERE rowid = ?1", [id])?;
                report.updated += 1;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO library_entries (root_id, path, rel, title, ext, size, mtime, created, body, indexed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![root_id, path, f.rel, title, f.ext, f.size, f.mtime, f.created, body, now],
                )?;
                report.added += 1;
                tx.last_insert_rowid()
            }
        };
        let stems = text::stems(&format!("{title} {} {body}", f.rel)).join(" ");
        tx.execute(
            "INSERT INTO library_fts (rowid, stem) VALUES (?1, ?2)",
            params![id, stems],
        )?;
        for target in parse_links(&body) {
            tx.execute(
                "INSERT OR IGNORE INTO library_links (entry_id, target) VALUES (?1, ?2)",
                params![id, target],
            )?;
        }
    }
    for (path, (id, _, _)) in &known {
        if !seen.contains(path) {
            tx.execute("DELETE FROM library_links WHERE entry_id = ?1", [id])?;
            tx.execute("DELETE FROM library_fts WHERE rowid = ?1", [id])?;
            tx.execute("DELETE FROM library_entries WHERE id = ?1", [id])?;
            report.removed += 1;
        }
    }
    tx.execute(
        "UPDATE library_roots SET scanned_at = ?2 WHERE id = ?1",
        params![root_id, now],
    )?;
    tx.commit()?;
    Ok(report)
}

/// Rescans every folder mounted on this space.
pub fn rescan_all(conn: &Connection, now: i64) -> Result<ScanReport> {
    let mut total = ScanReport::default();
    for r in roots(conn)? {
        let one = rescan(conn, r.id, now)?;
        total.added += one.added;
        total.updated += one.updated;
        total.unchanged += one.unchanged;
        total.removed += one.removed;
    }
    Ok(total)
}

/// A few words of the text around the first word of the query found in it, else its start.
fn snippet(body: &str, query: &str) -> String {
    let chars: Vec<char> = body.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let mut at = 0;
    'words: for w in query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
    {
        let w: Vec<char> = w
            .chars()
            .map(|c| c.to_lowercase().next().unwrap_or(c))
            .collect();
        if let Some(i) = lower.windows(w.len()).position(|win| win == w.as_slice()) {
            at = i;
            break 'words;
        }
    }
    let start = at.saturating_sub(60);
    let end = (at + 140).min(chars.len());
    let mut s: String = chars[start..end]
        .iter()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

fn fts_query(stems: &[String]) -> Option<String> {
    let mut seen = BTreeSet::new();
    let terms: Vec<String> = stems
        .iter()
        .filter(|s| seen.insert((*s).clone()))
        .map(|s| format!("\"{}\"", s.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// Looks the index up by words (accents and plurals ignored). Best first, at most `limit` (default 10, up to 50).
pub fn search(conn: &Connection, query: &str, limit: usize) -> Result<Vec<LibHit>> {
    let Some(q) = fts_query(&text::stems(query)) else {
        return Ok(vec![]);
    };
    let limit = if limit == 0 { 10 } else { limit.min(50) };
    let mut st = conn.prepare(
        "SELECT e.path, e.rel, e.title, e.ext, e.mtime, e.created, e.body
         FROM library_fts JOIN library_entries e ON e.id = library_fts.rowid
         WHERE library_fts MATCH ?1 ORDER BY bm25(library_fts) LIMIT ?2",
    )?;
    let rows = st.query_map(params![q, limit as i64], |r| {
        let body: String = r.get(6)?;
        Ok(LibHit {
            path: r.get(0)?,
            rel: r.get(1)?,
            title: r.get(2)?,
            ext: r.get(3)?,
            modified: r.get(4)?,
            created: r.get(5)?,
            snippet: snippet(&body, query),
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// What raising an original gave.
#[derive(Clone, Debug)]
pub enum Raised {
    /// The fact written (or the identical one already active).
    Fact(Box<facts::Fact>),
    /// Close facts exist: nothing was written; say which are different with `distinct_from`.
    NeedsDecision(Vec<Candidate>),
}

/// What a caller gives to raise an original.
#[derive(Clone, Copy, Debug)]
pub struct RaiseRequest<'a> {
    /// Path of an indexed file: absolute, or inside the library when only one library holds it.
    pub path: &'a str,
    /// The short summary to store (at most 1200 characters).
    pub summary: &'a str,
    /// Tags of the new fact (`library/raised` is added).
    pub tags: &'a [String],
    /// Ids of close facts judged different, as for `remember`.
    pub distinct_from: &'a [String],
}

/// Raises an original: writes a normal fact in `space` with the caller's `summary` and `tags`, and links it to the
/// original, which stays where it is. `path` is the path of an indexed file (its absolute path, or its path inside
/// the library when only one has it). The fact is a plain fact: sorted, corrected and retired like any other; it is
/// never put in the inbox.
pub fn raise(
    conn: &Connection,
    space: &Space,
    author: &str,
    req: &RaiseRequest,
    now: i64,
) -> Result<Raised> {
    let RaiseRequest {
        path,
        summary,
        tags,
        distinct_from,
    } = *req;
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(Error::Invalid(
            "summary: say in a few lines what it is".into(),
        ));
    }
    if summary.chars().count() > MAX_SUMMARY {
        return Err(Error::Invalid(format!(
            "summary: at most {MAX_SUMMARY} characters (a summary, not a copy)"
        )));
    }
    let mut all = tags.to_vec();
    all.push(RAISED_TAG.into());
    let all = facts::normalize_tags(&all)?;
    if let Some(bad) = all.iter().find(|t| {
        *t == "inbox" || t.starts_with("inbox/") || *t == "sources" || t.starts_with("sources/")
    }) {
        return Err(Error::Invalid(format!(
            "tag {bad}: a raised fact is filed at once, never in the inbox or the sources"
        )));
    }
    let mut hits =
        conn.prepare("SELECT path, rel, root_id FROM library_entries WHERE path = ?1 OR rel = ?1")?;
    let rows: Vec<(String, String, i64)> = hits
        .query_map([path], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let (abs, rel, root_id) = match rows.as_slice() {
        [one] => one.clone(),
        [] => return Err(Error::Invalid("no such file in the library".into())),
        _ => {
            return Err(Error::Invalid(
                "several libraries hold that path: give the whole path".into(),
            ));
        }
    };
    let mut new = NewFact::agent(&format!("{summary}\n\nOriginal: {abs}"), author);
    new.tags = all;
    let fact = match facts::remember(conn, space, author, new, distinct_from)? {
        Remembered::Created(f) | Remembered::Duplicate(f) => f,
        Remembered::NeedsDecision(c) => return Ok(Raised::NeedsDecision(c)),
    };
    conn.execute(
        "INSERT OR REPLACE INTO library_raised (fact_id, root_id, path, rel, raised_at, author)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![fact.id, root_id, abs, rel, now, author],
    )?;
    Ok(Raised::Fact(Box::new(fact)))
}

/// A folder of a library, as the graph sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    /// The mount the folder belongs to.
    pub root_id: i64,
    /// The last part of the library's path.
    pub root_name: String,
    /// The folder inside the library, cut at the depth asked (empty: the top).
    pub folder: String,
    /// Files in it.
    pub entries: i64,
    /// `[[links]]` from here that lead to no file of the library.
    pub unresolved: i64,
}

/// What a library adds to the graph of links.
#[derive(Clone, Debug, Default)]
pub struct LibraryGraph {
    /// Folders, cut at the depth asked.
    pub folders: Vec<Folder>,
    /// `[[links]]` between two folders: (from, to) as indexes into `folders`, and how many.
    pub links: Vec<(usize, usize, i64)>,
    /// Active facts raised from a folder: (folder index, count).
    pub raised: Vec<(usize, i64)>,
}

fn folder_of(rel: &str, depth: usize) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    parts[..parts.len() - 1]
        .iter()
        .take(depth)
        .copied()
        .collect::<Vec<_>>()
        .join("/")
}

fn norm_target(t: &str) -> String {
    let t = t.trim().to_lowercase();
    t.strip_suffix(".md").unwrap_or(&t).to_string()
}

/// The folders of this space's libraries (cut at `depth` levels), the `[[links]]` between them, and the facts raised
/// from them.
pub fn graph(conn: &Connection, depth: usize) -> Result<LibraryGraph> {
    let roots = roots(conn)?;
    let name_of: HashMap<i64, String> = roots
        .iter()
        .map(|r| {
            let n = Path::new(&r.root)
                .file_name()
                .map_or_else(|| r.root.clone(), |n| n.to_string_lossy().into_owned());
            (r.id, n)
        })
        .collect();
    let mut st = conn.prepare("SELECT id, root_id, rel FROM library_entries ORDER BY rel, id")?;
    let entries: Vec<(i64, i64, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut out = LibraryGraph::default();
    let mut index: BTreeMap<(i64, String), usize> = BTreeMap::new();
    let mut slot = |out: &mut LibraryGraph, root_id: i64, folder: String| -> usize {
        *index.entry((root_id, folder.clone())).or_insert_with(|| {
            out.folders.push(Folder {
                root_id,
                root_name: name_of.get(&root_id).cloned().unwrap_or_default(),
                folder,
                entries: 0,
                unresolved: 0,
            });
            out.folders.len() - 1
        })
    };
    let mut folder_of_entry: HashMap<i64, usize> = HashMap::new();
    // Resolution maps, by library: a file name, or the path inside the library, without `.md`.
    let mut by_stem: HashMap<(i64, String), i64> = HashMap::new();
    let mut by_path: Vec<(i64, String, i64)> = vec![];
    for (id, root_id, rel) in &entries {
        let f = slot(&mut out, *root_id, folder_of(rel, depth));
        out.folders[f].entries += 1;
        folder_of_entry.insert(*id, f);
        by_stem
            .entry((*root_id, norm_target(stem_of(rel))))
            .or_insert(*id);
        by_path.push((*root_id, norm_target(&rel.to_lowercase()), *id));
    }
    let root_of: HashMap<i64, i64> = entries.iter().map(|(id, r, _)| (*id, *r)).collect();
    let mut counts: BTreeMap<(usize, usize), i64> = BTreeMap::new();
    let mut st =
        conn.prepare("SELECT entry_id, target FROM library_links ORDER BY entry_id, target")?;
    let links: Vec<(i64, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for (from, target) in links {
        let (Some(&root_id), Some(&from_folder)) = (root_of.get(&from), folder_of_entry.get(&from))
        else {
            continue;
        };
        let t = norm_target(&target);
        let hit = if t.contains('/') {
            by_path
                .iter()
                .find(|(r, p, _)| *r == root_id && (*p == t || p.ends_with(&format!("/{t}"))))
                .map(|(_, _, id)| *id)
        } else {
            by_stem.get(&(root_id, t)).copied()
        };
        match hit.and_then(|id| folder_of_entry.get(&id).copied()) {
            Some(to) if to != from_folder => *counts.entry((from_folder, to)).or_insert(0) += 1,
            Some(_) => {}
            None => out.folders[from_folder].unresolved += 1,
        }
    }
    out.links = counts.into_iter().map(|((a, b), n)| (a, b, n)).collect();
    let mut st = conn.prepare(
        "SELECT r.root_id, r.rel FROM library_raised r JOIN facts f ON f.id = r.fact_id WHERE f.status = 'active'",
    )?;
    let raised: Vec<(i64, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut rc: BTreeMap<usize, i64> = BTreeMap::new();
    for (root_id, rel) in raised {
        let f = slot(&mut out, root_id, folder_of(&rel, depth));
        *rc.entry(f).or_insert(0) += 1;
    }
    out.raised = rc.into_iter().collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_read_with_their_alias_and_heading() {
        let l = parse_links("See [[Note A]] and [[b/c|alias]], ![[img#h]]\nnot [[ ]] or [[broken");
        assert_eq!(
            l.into_iter().collect::<Vec<_>>(),
            vec!["Note A", "b/c", "img"]
        );
    }

    #[test]
    fn a_title_comes_from_the_front_matter_or_the_first_heading() {
        assert_eq!(
            title_of("---\ntitle: \"Hi\"\n---\n# Other").as_deref(),
            Some("Hi")
        );
        assert_eq!(
            title_of("---\ntags: x\n---\n\n# Heading\ntext").as_deref(),
            Some("Heading")
        );
        assert_eq!(title_of("no heading"), None);
    }

    #[test]
    fn a_folder_is_cut_at_the_depth() {
        assert_eq!(folder_of("a/b/c.md", 1), "a");
        assert_eq!(folder_of("a/b/c.md", 2), "a/b");
        assert_eq!(folder_of("c.md", 3), "");
    }
}
